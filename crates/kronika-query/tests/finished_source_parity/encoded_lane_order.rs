use super::*;

use kronika_query::{ActiveCursor, QueryError, SegmentBounds, SegmentSelection};
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV3;

#[path = "../../../../bins/kronika-web/src/query_adapter.rs"]
mod query_adapter;

const fn at(seconds: i64) -> i64 {
    SEGMENT_ID + seconds * 1_000_000
}

#[expect(
    clippy::too_many_lines,
    reason = "one encoded sample carries the actual context, counters, gauges and PG rows"
)]
fn lane_part(samples: &[(i64, &[u8], i64)], host_cpu: bool) -> Vec<u8> {
    let mut interner = Interner::new(DictLimits::default());
    let path = fixture_label(&mut interner, b"/selected");
    let root = fixture_label(&mut interner, b"/");
    let label = fixture_label(&mut interner, b"fixture");
    let backend = fixture_label(&mut interner, b"client backend");
    let active = fixture_label(&mut interner, b"active");
    let mut buffers = SectionBuffers::new();
    for &(seconds, identity, usage_usec) in samples {
        let ts = Ts(at(seconds));
        let identity = fixture_label(&mut interner, identity);
        buffers
            .push(OsCgroupContextV2 {
                ts,
                cgroup_version: 2,
                cpu_path: Some(path),
                memory_path: Some(path),
                io_path: None,
                cpuset_cpus: Some(2),
                effective_cpu_quota_usec: Some(200_000),
                effective_cpu_period_usec: Some(100_000),
                effective_memory_max: Some(1_000),
                pids_path: None,
                cpu_identity: Some(identity),
                memory_identity: Some(identity),
                io_identity: None,
                pids_identity: None,
                cpu_root: Some(root),
                memory_root: Some(root),
                io_root: None,
                pids_root: None,
                scope: 4,
            })
            .expect("selected controller context");
        buffers
            .push(OsCgroupCpuV3 {
                ts,
                cgroup_path: path,
                cgroup_identity: identity,
                usage_usec,
                user_usec: usage_usec,
                system_usec: 0,
                throttled_usec: None,
                nr_throttled: None,
                quota_usec: None,
                period_usec: None,
                scope: 4,
            })
            .expect("selected CPU counter");
        buffers
            .push(OsCgroupMemoryV3 {
                ts,
                cgroup_path: path,
                cgroup_identity: identity,
                current: 100 + seconds * 10,
                max: None,
                anon: None,
                file: None,
                kernel: None,
                slab: None,
                low_events: None,
                high_events: None,
                max_events: None,
                oom_events: None,
                oom_kill: Some(seconds),
                max_unlimited: None,
                scope: 4,
            })
            .expect("selected memory gauge and counter");
        buffers
            .push(PgStatActivityV3 {
                ts,
                pid: 42,
                leader_pid: None,
                datid: Some(1),
                datname: Some(label),
                usename: Some(label),
                application_name: label,
                client_addr: label,
                backend_type: backend,
                state: Some(active),
                wait_event_type: None,
                wait_event: None,
                query: Some(label),
                query_id: Some(71),
                backend_xid_age: None,
                backend_xmin_age: None,
                backend_start: Ts(SEGMENT_ID),
                xact_start: Some(Ts(SEGMENT_ID)),
                query_start: Some(Ts(SEGMENT_ID)),
                state_change: Some(Ts(SEGMENT_ID)),
            })
            .expect("ordinary PostgreSQL gauge");
        if host_cpu {
            buffers
                .push(InstanceMetadataV3 {
                    ts,
                    hostname: Some(label),
                    kernel_version: Some(label),
                    environment: Some(0),
                    clock_ticks_per_sec: Some(100),
                    page_size_bytes: Some(4096),
                    boot_id: Some(label),
                    btime: Some(Ts(1)),
                    os_enabled: true,
                    postgresql_processes_shared: false,
                    postgresql_enabled: true,
                    postgresql_interval_seconds: 30,
                    postgresql_effective_cpus: None,
                })
                .expect("recorded host CPU denominator");
            for (cpu_id, user) in [(-1, seconds * 100), (0, seconds * 50), (1, seconds * 50)] {
                buffers
                    .push(OsCpu {
                        ts,
                        cpu_id,
                        user,
                        nice: 0,
                        system: 0,
                        idle: 0,
                        iowait: 0,
                        irq: 0,
                        softirq: 0,
                        steal: 0,
                        guest: 0,
                        guest_nice: 0,
                        scope: 0,
                    })
                    .expect("host aggregate and physical cores");
            }
        }
    }
    let dictionary = dict::encode(interner.window()).expect("encode dictionary");
    buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("rows")
}

fn seal(root: &Path, id: i64, part: &[u8]) {
    let data_root = DataRoot::open(root).expect("root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let id = SegmentId::new(id).expect("id");
    journal.append(id, part).expect("append raw WAL part");
    write_segment(&journal, &owner, SegmentAddress::new(id).expect("address")).expect("seal ZMS");
    journal.reset().expect("reset journal");
}

const fn lane_request(ids: Vec<i64>, active: Option<ActiveCursor>) -> HourRequest {
    HourRequest {
        window: Window {
            from: Some(at(0)),
            to: Some(at(60)),
        },
        series: None,
        part: HourPart::Lanes,
        segments: Some(ids),
        active,
    }
}

fn lane_values(records: &[serde_json::Value], key: &str) -> Vec<(i64, Option<f64>)> {
    records
        .iter()
        .filter(|row| row["record"] == "lane" && row["lane"] == key)
        .map(|row| {
            let ts = row["ts"]
                .as_str()
                .expect("timestamp")
                .parse()
                .expect("integer ts");
            (ts, row["value"].as_f64())
        })
        .collect()
}

fn assert_ordinary_gauges(records: &[serde_json::Value], seconds: &[i64]) {
    assert_eq!(
        lane_values(records, "pg_running"),
        seconds
            .iter()
            .map(|&s| (at(s), Some(1.0)))
            .collect::<Vec<_>>(),
        "one PG count per sample, including the retained predecessor"
    );
    #[expect(
        clippy::cast_precision_loss,
        reason = "small exact fixture gauge values"
    )]
    let expected = seconds
        .iter()
        .map(|&s| (at(s), Some((100 + s * 10) as f64)))
        .collect::<Vec<_>>();
    assert_eq!(
        lane_values(records, "cg_memory_bytes"),
        expected,
        "one recorded memory gauge per sample across segment boundaries"
    );
    let identities = records
        .iter()
        .filter(|row| row["record"] == "lane")
        .map(|row| {
            (
                row["lane"].as_str().expect("lane"),
                row["ts"].as_str().expect("ts"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        identities
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        identities.len(),
        "no lane/timestamp is re-emitted"
    );
}

#[test]
fn encoded_overlap_orders_identities_and_preserves_deferred_host_cpu() {
    let directory = tempfile::tempdir().expect("recording");
    seal(
        directory.path(),
        at(10),
        &lane_part(&[(10, b"A", 10_000_000), (30, b"B", 90_000_000)], true),
    );
    // The later-admitted segment supplies an earlier observation. It has no
    // host CPU facts, so finalizing the deferred host sample must use S1 facts.
    seal(
        directory.path(),
        at(20),
        &lane_part(&[(20, b"A", 20_000_000)], false),
    );
    let dataset = Arc::new(FinishedDataset::new(
        PosixSource::open(directory.path()).expect("POSIX source"),
    ));
    let records = ndjson(&hour_bytes(
        dataset,
        lane_request(vec![at(10), at(20)], None),
    ));
    assert_eq!(
        lane_values(&records, "cg_cpu_cores"),
        [(at(10), None), (at(20), Some(1.0)), (at(30), None)]
    );
    assert_eq!(
        lane_values(&records, "cg_cpu_share"),
        [(at(10), None), (at(20), Some(50.0)), (at(30), None)]
    );
    assert_eq!(
        lane_values(&records, "cg_oom"),
        [(at(10), None), (at(20), Some(1.0)), (at(30), None)]
    );
    assert_eq!(
        lane_values(&records, "cpu_busy"),
        [(at(10), None), (at(30), Some(50.0))]
    );
    assert_ordinary_gauges(&records, &[10, 20, 30]);
}

#[test]
fn encoded_compact_segments_emit_pg_and_gauges_once() {
    let directory = tempfile::tempdir().expect("recording");
    for seconds in [10, 20, 30] {
        seal(
            directory.path(),
            at(seconds),
            &lane_part(&[(seconds, b"A", seconds * 1_000_000)], false),
        );
    }
    let dataset = Arc::new(FinishedDataset::new(
        PosixSource::open(directory.path()).expect("POSIX source"),
    ));
    let records = ndjson(&hour_bytes(
        dataset,
        lane_request(vec![at(10), at(20), at(30)], None),
    ));
    assert_eq!(
        lane_values(&records, "cg_cpu_cores"),
        [(at(10), None), (at(20), Some(1.0)), (at(30), Some(1.0))]
    );
    assert_ordinary_gauges(&records, &[10, 20, 30]);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the complete production WAL capture and both invalid cursor checks share one fixture"
)]
fn encoded_active_prefix_reorders_after_validating_current_catalog_ids() {
    let directory = tempfile::tempdir().expect("recording");
    seal(
        directory.path(),
        at(0),
        &lane_part(&[(0, b"A", 0), (40, b"A", 40_000_000)], false),
    );
    seal(
        directory.path(),
        at(20),
        &lane_part(&[(20, b"A", 20_000_000)], false),
    );
    let data_root = DataRoot::open(directory.path()).expect("root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let active_id = SEGMENT_ID + 1;
    journal
        .append(
            SegmentId::new(active_id).expect("active id"),
            &lane_part(&[(50, b"A", 50_000_000)], false),
        )
        .expect("first committed frame");
    let reader = Reader::open(directory.path()).expect("reader");
    let before = reader.segments(..).expect("prefix listing");
    let prefix = before
        .segments
        .iter()
        .find(|segment| segment.id() == active_id)
        .expect("active prefix");
    let position = prefix.active_position().expect("committed WAL position");
    assert_eq!(prefix.min_ts(), at(50));
    journal
        .append(
            SegmentId::new(active_id).expect("active id"),
            &lane_part(&[(10, b"A", 10_000_000)], false),
        )
        .expect("later admitted earlier observation");
    let dataset: Arc<dyn QueryDataset> = Arc::new(
        query_adapter::NativeDataset::from_root(directory.path()).expect("production adapter"),
    );
    let catalog = dataset.catalog().expect("catalog");
    let mut listed = catalog
        .segments(SegmentSelection::new(SegmentBounds::all()))
        .expect("descriptors")
        .segments;
    listed.sort_by_key(kronika_query::DatasetSegment::min_ts);
    assert_eq!(
        listed
            .iter()
            .map(|segment| (segment.id(), segment.min_ts()))
            .collect::<Vec<_>>(),
        [(at(0), at(0)), (active_id, at(10)), (at(20), at(20))]
    );
    let pinned = dataset
        .at_active_position(&listed[1], position)
        .expect("valid old raw WAL prefix");
    assert_eq!((pinned.min_ts(), pinned.max_ts()), (at(50), at(50)));
    assert_eq!(
        dataset.open(&pinned).expect("decode pinned rows").min_ts(),
        at(50)
    );
    drop(catalog);
    let cursor = Some(ActiveCursor {
        segment_id: active_id,
        wal_position: position,
    });
    let request = lane_request(vec![at(0), active_id, at(20)], cursor);
    let records = ndjson(&hour_bytes(Arc::clone(&dataset), request));
    assert_eq!(
        lane_values(&records, "cg_cpu_cores"),
        [
            (at(0), None),
            (at(20), Some(1.0)),
            (at(40), Some(1.0)),
            (at(50), Some(1.0))
        ]
    );
    assert_ordinary_gauges(&records, &[0, 20, 40, 50]);
    let context = QueryContext::new(dataset, 0b11, false);
    assert!(
        matches!(
            execute(
                &context,
                QueryRequest::Hour(lane_request(vec![at(0), at(20), active_id], cursor))
            ),
            Err(QueryError::BadCursor)
        ),
        "IDs must match the current unpinned catalog order"
    );
    assert!(
        matches!(
            execute(
                &context,
                QueryRequest::Hour(lane_request(
                    vec![at(0), active_id, at(20)],
                    Some(ActiveCursor {
                        segment_id: active_id,
                        wal_position: position - 1
                    })
                ))
            ),
            Err(QueryError::BadCursor)
        ),
        "a non-frame WAL position remains invalid"
    );
}
