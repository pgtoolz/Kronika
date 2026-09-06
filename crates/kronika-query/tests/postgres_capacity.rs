//! Recorded `PostgreSQL` capacity through WAL, ZMS and embedded report queries.

use std::sync::Arc;

use base64 as _;
use icu_collator as _;
use icu_locale_core as _;
use kronika_format::DictLimits;
use kronika_index::{Index, SeriesBlock, build, build_from_reader};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_query::{
    FinishedDataset, IndexRequest, MemoryIndexProvider, QueryContext, QueryRequest, QuerySink,
    execute,
};
use kronika_reader::{FinishedReader, Reader, SegmentKind};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_cgroup_context::OsCgroupContext;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::{StrId, Ts};
use kronika_store::EmbeddedSource;
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde as _;

const START: i64 = 1_709_164_800_000_000;
const ACTIVITY: u32 = 1_001_004;

type HealthValues = Vec<(i64, Option<u8>)>;

struct Fixture {
    environment: Environment,
    explicit_cpus: Option<u32>,
    cpu_snapshots: Vec<(i64, Vec<i32>)>,
    contexts: Vec<OsCgroupContext>,
    active: Vec<(i64, u32)>,
}

impl Fixture {
    const fn machine(cpu_snapshots: Vec<(i64, Vec<i32>)>, active: Vec<(i64, u32)>) -> Self {
        Self {
            environment: Environment::Machine,
            explicit_cpus: None,
            cpu_snapshots,
            contexts: Vec::new(),
            active,
        }
    }

    fn container(contexts: Vec<OsCgroupContext>, active: Vec<(i64, u32)>) -> Self {
        Self {
            environment: Environment::Container,
            explicit_cpus: None,
            // Node CPUs are deliberately present to catch container fallback.
            cpu_snapshots: vec![(START, (-1..8).collect())],
            contexts,
            active,
        }
    }
}

const fn context(
    at: i64,
    quota: Option<i64>,
    period: Option<i64>,
    cpuset: Option<i64>,
) -> OsCgroupContext {
    OsCgroupContext {
        ts: Ts(at),
        cgroup_version: 2,
        cpu_path: None,
        memory_path: None,
        io_path: None,
        cpuset_cpus: cpuset,
        effective_cpu_quota_usec: quota,
        effective_cpu_period_usec: period,
        effective_memory_max: None,
        scope: 3,
    }
}

const fn cpu(at: i64, cpu_id: i32) -> OsCpu {
    OsCpu {
        ts: Ts(at),
        cpu_id,
        user: 0,
        nice: 0,
        system: 0,
        idle: 100,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
        guest: 0,
        guest_nice: 0,
        scope: 0,
    }
}

const fn activity(at: i64, pid: i32, state: StrId) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(at),
        pid,
        leader_pid: None,
        datid: None,
        datname: None,
        usename: None,
        application_name: state,
        client_addr: state,
        backend_type: state,
        state: Some(state),
        wait_event_type: None,
        wait_event: None,
        query: None,
        query_id: None,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Ts(START),
        xact_start: None,
        query_start: None,
        state_change: None,
    }
}

fn append(journal: &mut Journal, id: SegmentId, fixture: &Fixture) {
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(interner.intern(b"recorded").expect("fixture label").get());
    let active = StrId(interner.intern(b"active").expect("active state").get());
    let idle = StrId(interner.intern(b"idle").expect("idle state").get());
    let path = StrId(interner.intern(b"/recorded").expect("cgroup path").get());
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(id.get()),
            hostname: label,
            kernel_version: label,
            environment: fixture.environment.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4_096,
            boot_id: label,
            btime: Ts(START - 100_000_000),
            postgresql_enabled: true,
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: fixture.explicit_cpus,
        })
        .expect("metadata");
    for (at, ids) in &fixture.cpu_snapshots {
        for &cpu_id in ids {
            buffers.push(cpu(*at, cpu_id)).expect("CPU row");
        }
    }
    for &row in &fixture.contexts {
        buffers
            .push(OsCgroupContext {
                cpu_path: Some(path),
                ..row
            })
            .expect("cgroup context");
    }
    for &(at, count) in &fixture.active {
        for pid in 1..=count {
            buffers
                .push(activity(at, i32::try_from(pid).expect("PID fits"), active))
                .expect("active backend");
        }
        buffers
            .push(activity(at, 0, idle))
            .expect("idle backend represents zero-active snapshot");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode WAL part")
        .expect("nonempty WAL part");
    journal.append(id, &part).expect("append WAL part");
}

fn health(index: &Index) -> HealthValues {
    index
        .blocks
        .iter()
        .find_map(|block| match block {
            SeriesBlock::PostgresHealth(points) => Some(
                points
                    .iter()
                    .map(|point| (point.timestamp, point.value))
                    .collect(),
            ),
            _ => None,
        })
        .expect("PostgreSQL Health block")
}

fn active_findings(index: &Index) -> Vec<i64> {
    index
        .blocks
        .iter()
        .filter_map(|block| match block {
            SeriesBlock::Findings(block) if block.type_id == ACTIVITY => Some(block),
            _ => None,
        })
        .flat_map(|block| block.findings.iter().map(|finding| finding.timestamp))
        .collect()
}

#[derive(Default)]
struct Records(Vec<serde_json::Value>);

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0
            .push(serde_json::from_slice(&bytes).expect("query record"));
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn check_artifacts(fixture: &Fixture, expected: &[(i64, Option<u8>)], hits: &[i64]) {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let id = SegmentId::new(START).expect("segment id");
    append(&mut journal, id, fixture);
    let reader = Reader::open(directory.path()).expect("WAL reader");
    let listing = reader.segments(..).expect("WAL listing");
    let active_ref = listing.segments.first().expect("WAL segment");
    assert_eq!(
        active_ref.kind(),
        SegmentKind::Active,
        "fixture is live WAL"
    );
    let segment = reader.open_segment(active_ref).expect("decode WAL");
    let wal_index = build_from_reader(&reader, active_ref, &segment).expect("WAL index");
    assert_eq!(health(&wal_index), expected, "WAL Health");
    assert_eq!(active_findings(&wal_index), hits, "WAL active findings");

    let address = SegmentAddress::new(id).expect("segment address");
    write_segment(&journal, &writer, address).expect("write ZMS");
    journal.reset().expect("reset journal after writing ZMS");
    let reader = Reader::open(directory.path()).expect("ZMS reader");
    let listing = reader.segments(..).expect("ZMS listing");
    let finished_ref = listing.segments.first().expect("finished segment");
    assert_eq!(
        finished_ref.kind(),
        SegmentKind::Finished,
        "fixture is finished ZMS"
    );
    let segment = reader.open_segment(finished_ref).expect("decode ZMS");
    let zms_index = build_from_reader(&reader, finished_ref, &segment).expect("ZMS index");
    assert_eq!(health(&zms_index), expected, "ZMS Health");
    assert_eq!(active_findings(&zms_index), hits, "ZMS active findings");

    let path = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    let payload = std::fs::read(path).expect("ZMS payload");
    let length = u64::try_from(payload.len()).expect("payload length fits");
    let source = EmbeddedSource::from_owned(id, payload, length).expect("embedded ZMS");
    let embedded_reader = FinishedReader::new(source.clone());
    let resources = embedded_reader.resources().expect("embedded catalog");
    let resource = resources.resources.first().expect("embedded resource");
    let segment = embedded_reader
        .open_segment(resource)
        .expect("decode embedded ZMS");
    let report_index = build(&segment).expect("report index builder");
    let encoded = report_index.encode().expect("report IDX encoding");
    let decoded = Index::decode(&encoded).expect("report IDX decoding");
    assert_eq!(health(&decoded), expected, "report IDX Health");
    assert_eq!(active_findings(&decoded), hits, "report active findings");

    let provider = MemoryIndexProvider::new(id, encoded).expect("report index provider");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false)
        .with_index_provider(Arc::new(provider));
    let execution = execute(
        &context,
        QueryRequest::Index(IndexRequest {
            segment_id: START,
            section: "health".to_owned(),
        }),
    )
    .expect("embedded Health query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("embedded Health response");
    let actual: Vec<_> = records
        .0
        .iter()
        .filter(|record| record["series"] == "postgres_health")
        .map(|record| (record["ts"].clone(), record["value"].clone()))
        .collect();
    let expected: Vec<_> = expected
        .iter()
        .map(|(at, value)| (serde_json::json!(at.to_string()), serde_json::json!(value)))
        .collect();
    assert_eq!(actual, expected, "embedded query Health");
}

#[test]
fn machine_capacity_uses_eight_recorded_cpus() {
    check_artifacts(
        &Fixture::machine(
            vec![(START, (-1..8).collect())],
            vec![(START, 0), (START + 10, 16), (START + 20, 20)],
        ),
        &[
            (START, Some(100)),
            (START + 10, Some(100)),
            (START + 20, Some(80)),
        ],
        &[START + 20],
    );
}

#[test]
fn machine_topology_uses_each_snapshot_and_distinct_nonnegative_ids() {
    check_artifacts(
        &Fixture::machine(
            vec![
                (START + 10, (-1..8).collect()),
                (START + 30, vec![-1, 2, 2, 17]),
                (START + 50, vec![-1]),
            ],
            vec![
                (START, 20),
                (START + 20, 20),
                (START + 40, 5),
                (START + 60, 5),
            ],
        ),
        &[
            (START, None),
            (START + 20, Some(80)),
            (START + 40, Some(80)),
            (START + 60, None),
        ],
        &[START + 20, START + 40],
    );
}

#[test]
fn fractional_quota_changes_affect_only_subsequent_activity() {
    check_artifacts(
        &Fixture::container(
            vec![
                context(START + 10, Some(150_000), Some(100_000), Some(8)),
                context(START + 40, Some(200_000), Some(100_000), Some(8)),
            ],
            vec![
                (START, 4),
                (START + 20, 3),
                (START + 30, 4),
                (START + 40, 4),
            ],
        ),
        &[
            (START, None),
            (START + 20, Some(100)),
            (START + 30, Some(75)),
            (START + 40, Some(100)),
        ],
        &[START + 30],
    );
}

#[test]
fn active_finding_uses_exact_capacity_even_when_health_rounds_to_one_hundred() {
    check_artifacts(
        &Fixture::container(
            vec![context(START, Some(299_999), Some(200_000), Some(8))],
            vec![(START, 3)],
        ),
        &[(START, Some(100))],
        &[START],
    );
}

#[test]
fn cpuset_caps_quota_and_bounds_known_unlimited_quota() {
    check_artifacts(
        &Fixture::container(
            vec![
                context(START, Some(400_000), Some(100_000), Some(2)),
                context(START + 20, Some(-1), Some(100_000), Some(2)),
                context(START + 40, Some(150_000), Some(100_000), None),
            ],
            vec![(START + 10, 5), (START + 30, 5), (START + 50, 4)],
        ),
        &[
            (START + 10, Some(80)),
            (START + 30, Some(80)),
            (START + 50, Some(75)),
        ],
        &[START + 10, START + 30, START + 50],
    );
}

#[test]
fn unknown_invalid_and_unbounded_context_never_use_node_cpus() {
    check_artifacts(
        &Fixture::container(
            vec![
                context(START, None, Some(100_000), Some(2)),
                context(START + 10, Some(0), Some(100_000), Some(2)),
                context(START + 20, Some(150_000), None, Some(2)),
                context(START + 30, Some(150_000), Some(0), Some(2)),
                context(START + 40, Some(-2), Some(100_000), Some(2)),
                context(START + 50, Some(-1), Some(100_000), None),
                OsCgroupContext {
                    scope: 4,
                    ..context(START + 60, Some(150_000), Some(100_000), Some(2))
                },
            ],
            (0..7).map(|step| (START + step * 10, 5)).collect(),
        ),
        &(0..7)
            .map(|step| (START + step * 10, None))
            .collect::<Vec<_>>(),
        &[],
    );
}

#[test]
fn explicit_target_capacity_overrides_recorded_container_capacity() {
    let mut fixture = Fixture::container(
        vec![context(START, Some(150_000), Some(100_000), Some(8))],
        vec![(START + 10, 4), (START + 20, 6), (START + 30, 8)],
    );
    fixture.explicit_cpus = Some(3);
    check_artifacts(
        &fixture,
        &[
            (START + 10, Some(100)),
            (START + 20, Some(100)),
            (START + 30, Some(75)),
        ],
        &[START + 30],
    );
}

#[test]
fn remote_target_capacity_overrides_collector_vm_capacity() {
    let mut fixture = Fixture::machine(
        vec![(START, (-1..8).collect())],
        vec![(START + 10, 8), (START + 20, 10)],
    );
    fixture.explicit_cpus = Some(4);
    check_artifacts(
        &fixture,
        &[(START + 10, Some(100)), (START + 20, Some(80))],
        &[START + 20],
    );
}

#[test]
fn reader_uses_prior_recorded_capacity_until_current_context_arrives() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    let prior_id = SegmentId::new(START).expect("prior segment id");
    append(
        &mut journal,
        prior_id,
        &Fixture::container(
            vec![context(START, Some(150_000), Some(100_000), Some(8))],
            vec![(START + 20, 4)],
        ),
    );
    write_segment(
        &journal,
        &writer,
        SegmentAddress::new(prior_id).expect("prior address"),
    )
    .expect("write prior ZMS");
    journal.reset().expect("reset prior journal");
    let current_id = SegmentId::new(START + 100).expect("current segment id");
    let mut current = Fixture::container(
        vec![context(START + 110, Some(200_000), Some(100_000), Some(8))],
        vec![(START + 100, 4), (START + 110, 4)],
    );
    current.cpu_snapshots.clear();
    append(&mut journal, current_id, &current);
    let reader = Reader::open(directory.path()).expect("reader");
    let listing = reader
        .catalog_segment(current_id.get())
        .expect("current listing");
    let reference = listing.segments.first().expect("current reference");
    let segment = reader.open_segment(reference).expect("current WAL");
    let index = build_from_reader(&reader, reference, &segment).expect("reader index");
    assert_eq!(
        health(&index),
        [(START + 100, Some(75)), (START + 110, Some(100))]
    );
    assert_eq!(active_findings(&index), [START + 100]);
    write_segment(
        &journal,
        &writer,
        SegmentAddress::new(current_id).expect("current address"),
    )
    .expect("write current ZMS");
    journal.reset().expect("reset current journal");
    let reader = Reader::open(directory.path()).expect("finished reader");
    let listing = reader
        .catalog_segment(current_id.get())
        .expect("current listing");
    let reference = listing.segments.first().expect("current reference");
    let segment = reader.open_segment(reference).expect("current ZMS");
    let index = build_from_reader(&reader, reference, &segment).expect("finished reader index");
    assert_eq!(
        health(&index),
        [(START + 100, Some(75)), (START + 110, Some(100))]
    );
    assert_eq!(active_findings(&index), [START + 100]);
}
