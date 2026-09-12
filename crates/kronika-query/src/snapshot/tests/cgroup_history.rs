use super::*;
use crate::{HourPart, HourRequest, HourSeriesRequest, Window};
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;

fn cpu(ts: i64, path: StrId) -> OsCgroupV2Cpu {
    OsCgroupV2Cpu {
        ts: Ts(ts),
        cgroup_path: path,
        cgroup_identity: path,
        usage_usec: Some(ts),
        user_usec: Some(ts),
        system_usec: Some(0),
        nr_periods: Some(ts),
        nr_throttled: Some(ts),
        throttled_usec: Some(ts),
        quota_usec: None,
        period_usec: None,
        cpuset_cpus: None,
    }
}

fn selected_history(
    context: &QueryContext,
    section: &str,
    type_id: u32,
    field: &str,
    filters: Vec<Filter>,
) -> Vec<Value> {
    let request = HourRequest {
        window: Window {
            from: Some(0),
            to: Some(300),
        },
        series: Some(HourSeriesRequest {
            section: section.to_owned(),
            fields: vec![field.to_owned()],
            filters,
            type_id: Some(type_id),
            group: None,
            scope: StatementScope::All,
        }),
        part: HourPart::Combined,
        segments: None,
        active: None,
    };
    let mut records = SnapshotRecords::default();
    execute(context, QueryRequest::Hour(request))
        .expect("prepare history")
        .stream(&mut records)
        .expect("history");
    let mut rows = records
        .0
        .into_iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| {
        row["timestamp"]
            .as_str()
            .expect("timestamp")
            .parse::<i64>()
            .expect("number")
    });
    rows
}

fn context(payload: &Arc<[u8]>) -> QueryContext {
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("id"),
        payload.to_vec(),
        u64::try_from(payload.len()).expect("size"),
    )
    .expect("source");
    QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false)
}

fn filters() -> Vec<Filter> {
    ["cgroup_path", "cgroup_identity"]
        .into_iter()
        .map(|column| Filter {
            column: column.to_owned(),
            value: "/work".to_owned(),
        })
        .collect()
}

#[test]
fn filtered_cgroup_history_matches_snapshot_family_and_missing_predecessor_breaks() {
    for legacy_between in [false, true] {
        let payload = fixture_payload(|interner, buffers| {
            let path = StrId(interner.intern(b"/work").expect("path").get());
            for ts in [50, 150, 200] {
                buffers.push(cpu(ts, path)).expect("selected CPU");
            }
            if legacy_between {
                buffers
                    .push(OsCgroupCpu {
                        ts: Ts(100),
                        cgroup_path: path,
                        usage_usec: 100,
                        user_usec: 100,
                        system_usec: 0,
                        throttled_usec: 100,
                        nr_throttled: 0,
                        quota_usec: -1,
                        period_usec: 100_000,
                        scope: 1,
                    })
                    .expect("legacy CPU");
            } else {
                let other = StrId(interner.intern(b"/other").expect("other path").get());
                buffers.push(cpu(100, other)).expect("partial other group");
            }
        });
        let rows = selected_history(
            &context(&payload),
            "os_cgroup_v2_cpu",
            1_207_001,
            "throttled_usec",
            filters(),
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["timestamp"], "150");
        assert_eq!(rows[1]["break_before"], true);
        assert!(
            rows[2].get("break_before").is_none(),
            "continuous next observation"
        );
        let mut request = snapshot_request("os_cgroup_v2_cpu", &["throttled_interval"]);
        request.at = 150;
        request.filters = filters();
        let snapshot = snapshot_records(&payload, request);
        let row = snapshot
            .iter()
            .find(|record| record["record"] == "row")
            .expect("selected snapshot");
        assert!(
            row["values"][0].is_null(),
            "history preserves actual snapshot missing-predecessor semantics"
        );
    }
}

#[test]
fn filtered_pid_history_marks_a_hidden_source_change() {
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        for (ts, events_source) in [(50, 1), (100, 2), (150, 1), (200, 1)] {
            buffers
                .push(OsCgroupV2Pids {
                    ts: Ts(ts),
                    cgroup_path: path,
                    cgroup_identity: path,
                    current: Some(1),
                    max: None,
                    max_unlimited: Some(true),
                    failure_max: Some(ts),
                    events_source,
                })
                .expect("pids");
        }
    });
    let mut selected = filters();
    selected.push(Filter {
        column: "events_source".to_owned(),
        value: "1".to_owned(),
    });
    let rows = selected_history(
        &context(&payload),
        "os_cgroup_v2_pids",
        1_209_001,
        "failure_max",
        selected,
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["timestamp"], "150");
    assert_eq!(rows[1]["break_before"], true);
    assert!(rows[2].get("break_before").is_none());
}

#[test]
fn selected_cgroup_history_preserves_valid_cross_segment_continuity() {
    let root = tempfile::tempdir().expect("root");
    let owner = DataRoot::open(root.path())
        .expect("root")
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    for (offset, ts) in [(0, 50), (1, 150)] {
        let mut interner = Interner::new(DictLimits::default());
        let path = StrId(interner.intern(b"/work").expect("path").get());
        let mut buffers = SectionBuffers::new();
        buffers.push(cpu(ts, path)).expect("CPU");
        let dictionary = dict::encode(interner.window()).expect("dictionary");
        let part = buffers.flush(&dictionary).expect("encode").expect("part");
        let id = SegmentId::new(SEGMENT_ID + offset).expect("id");
        journal.append(id, &part).expect("WAL append");
        write_segment(&journal, &owner, SegmentAddress::new(id).expect("address"))
            .expect("seal ZMS");
        journal.reset().expect("next segment");
    }
    drop(journal);
    drop(owner);
    let source = PosixSource::open(root.path()).expect("source");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    let rows = selected_history(
        &context,
        "os_cgroup_v2_cpu",
        1_207_001,
        "throttled_usec",
        filters(),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["values"][0], "50");
    assert_eq!(rows[1]["values"][0], "150");
    assert!(rows.iter().all(|row| row.get("break_before").is_none()));
}
