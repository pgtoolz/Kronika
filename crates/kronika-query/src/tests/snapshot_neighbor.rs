use std::cell::Cell;
use std::sync::{Arc, Mutex};

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_stat_activity::{PgStatActivityV2, PgStatActivityV3};
use kronika_registry::{Section, StrId, Ts};
use kronika_store::PosixSource;
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use crate::{
    CapturedCatalog, DatasetListing, DatasetSegment, FinishedDataset, QueryContext, QueryDataset,
    QueryError, QueryRequest, QuerySink, QueryStability,
    SnapshotNeighborDirection::{Next, Previous},
    SnapshotNeighborRequest, SnapshotRequest, StatementScope, Window, execute,
};

const BASE: i64 = 1_709_164_800_000_000;

enum Sample {
    Activity(i64),
    LegacyActivity(i64),
    LegacyActivityPid(i64, i32),
    Host(i64),
    LockWait(i64),
    Graph(i64),
}

fn activity(at: i64, label: StrId) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(at),
        pid: 42,
        leader_pid: None,
        datid: None,
        datname: None,
        usename: None,
        application_name: label,
        client_addr: label,
        backend_type: label,
        state: None,
        wait_event_type: None,
        wait_event: None,
        query: None,
        query_id: Some(42),
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Ts(BASE),
        xact_start: None,
        query_start: None,
        state_change: None,
    }
}

fn push_sample(buffers: &mut SectionBuffers, label: StrId, lock: StrId, sample: &Sample) {
    match *sample {
        Sample::Activity(at) => buffers.push(activity(at, label)).expect("activity row"),
        Sample::LockWait(at) => {
            let mut row = activity(at, label);
            row.wait_event_type = Some(lock);
            buffers.push(row).expect("waiting row");
        }
        Sample::Graph(at) => buffers
            .push(PgLocksV2 {
                ts: Ts(at),
                pid: 42,
                blocked_by: vec![7, 0],
                datid: 1,
                datname: label,
                usename: None,
                application_name: label,
                client_addr: label,
                backend_type: label,
                state: None,
                wait_event_type: None,
                wait_event: None,
                query: label,
                backend_xid_age: None,
                backend_xmin_age: None,
                backend_start: None,
                xact_start: None,
                query_start: None,
                state_change: None,
                lock_locktype: None,
                lock_mode: None,
                lock_database: None,
                lock_relation: None,
                lock_relname: None,
                lock_page: None,
                lock_tuple: None,
                lock_virtualxid: None,
                lock_transactionid: None,
                lock_classid: None,
                lock_objid: None,
                lock_objsubid: None,
                lock_target: None,
                waitstart: None,
            })
            .expect("graph row"),
        Sample::LegacyActivity(at) | Sample::LegacyActivityPid(at, _) => {
            let row = activity(at, label);
            buffers
                .push(PgStatActivityV2 {
                    ts: row.ts,
                    pid: match *sample {
                        Sample::LegacyActivityPid(_, pid) => pid,
                        _ => row.pid,
                    },
                    leader_pid: row.leader_pid,
                    datname: row.datname,
                    usename: row.usename,
                    application_name: row.application_name,
                    client_addr: row.client_addr,
                    backend_type: row.backend_type,
                    state: row.state,
                    wait_event_type: row.wait_event_type,
                    wait_event: row.wait_event,
                    query: row.query,
                    backend_xid_age: row.backend_xid_age,
                    backend_xmin_age: row.backend_xmin_age,
                    backend_start: row.backend_start,
                    xact_start: row.xact_start,
                    query_start: row.query_start,
                    state_change: row.state_change,
                })
                .expect("legacy activity row");
        }
        Sample::Host(at) => buffers
            .push(OsLoadavg {
                ts: Ts(at),
                load1: 1.0,
                load5: 1.0,
                load15: 1.0,
                running: 1,
                total: 1,
                scope: 0,
            })
            .expect("host row"),
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    context: QueryContext,
}

fn fixture(segments: &[(i64, &[Sample])]) -> Fixture {
    let directory = tempfile::tempdir().expect("neighbor fixture directory");
    let root = DataRoot::open(directory.path()).expect("neighbor data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("neighbor writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("neighbor journal");
    for &(id, samples) in segments {
        let mut buffers = SectionBuffers::new();
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("fixture label").get());
        let lock = StrId(interner.intern(b"Lock").expect("Lock").get());
        for sample in samples {
            push_sample(&mut buffers, label, lock, sample);
        }
        let dictionary = dict::encode(interner.window()).expect("fixture dictionary");
        let part = buffers.flush(&dictionary).expect("part").expect("rows");
        let address = SegmentAddress::new(SegmentId::new(id).expect("id")).expect("address");
        journal.append(address.id, &part).expect("append");
        write_segment(&journal, &owner, address).expect("publish");
        journal.reset().expect("reset");
    }
    drop(journal);
    drop(owner);
    let source = PosixSource::open(directory.path()).expect("neighbor source");
    Fixture {
        _directory: directory,
        context: QueryContext::new(Arc::new(FinishedDataset::new(source)), 0, false),
    }
}

fn request(at: i64, direction: crate::SnapshotNeighborDirection) -> SnapshotNeighborRequest {
    SnapshotNeighborRequest {
        sections: vec!["pg_stat_activity".to_owned()],
        at,
        direction,
        window: Window::default(),
    }
}

#[derive(Default)]
struct Records {
    rows: Vec<Value>,
    cancelled: bool,
    cancel_after: Option<usize>,
    polls: Cell<usize>,
}

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.rows
            .push(serde_json::from_slice(&bytes).expect("JSON"));
        true
    }

    fn cancelled(&self) -> bool {
        self.polls.set(self.polls.get() + 1);
        self.cancelled
            || self
                .cancel_after
                .is_some_and(|limit| self.polls.get() >= limit)
    }
}

fn neighbor(context: &QueryContext, request: SnapshotNeighborRequest) -> Value {
    let execution = execute(context, QueryRequest::SnapshotNeighbor(request)).expect("prepare");
    assert_eq!(execution.metadata().stability(), QueryStability::Mutable);
    assert!(execution.metadata().identity().is_none());
    let mut records = Records::default();
    execution.stream(&mut records).expect("neighbor response");
    assert_eq!(records.rows.len(), 1);
    records.rows.remove(0)
}

fn found(at: i64, segment: i64) -> Value {
    json!({"record": "snapshot_neighbor", "at": at.to_string(), "segment_id": segment.to_string()})
}

fn absent() -> Value {
    json!({"record": "snapshot_neighbor", "at": null, "segment_id": null})
}

fn snapshot_request(segment_id: i64, at: i64) -> SnapshotRequest {
    SnapshotRequest {
        segment_id,
        latest: true,
        at,
        sections: vec!["pg_stat_activity".to_owned()],
        fields: vec!["pid".to_owned()],
        by: vec!["pid".to_owned()],
        direction: crate::Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: vec![],
        type_id: None,
        row_ordinal: None,
        scope: StatementScope::All,
    }
}

fn snapshot(context: &QueryContext, request: SnapshotRequest) -> Vec<Value> {
    let execution = execute(context, QueryRequest::Snapshot(request)).expect("prepare snapshot");
    let mut records = Records::default();
    execution.stream(&mut records).expect("snapshot response");
    records.rows
}

#[test]
fn ordinary_snapshot_resolves_the_neighbor_sample_despite_a_newer_preferred_segment() {
    let at = BASE + 20_000_000;
    let data = fixture(&[
        (BASE, &[Sample::Activity(at)]),
        (
            BASE + 1,
            &[
                Sample::Host(BASE),
                Sample::Activity(BASE + 10_000_000),
                Sample::Activity(BASE + 30_000_000),
            ],
        ),
    ]);
    assert_eq!(
        neighbor(&data.context, request(BASE + 18_000_000, Next)),
        found(at, BASE)
    );
    // The UI chooses this preferred segment from the catalog again on URL reload.
    let records = snapshot(&data.context, snapshot_request(BASE + 1, at));
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], at.to_string());
    assert_eq!(rows[0]["segment_id"], BASE.to_string());
    let mut anchored = snapshot_request(BASE + 1, at);
    anchored.latest = false;
    let records = snapshot(&data.context, anchored);
    let row = records
        .iter()
        .find(|record| record["record"] == "row")
        .expect("anchor row");
    assert_eq!(row["timestamp"], (BASE + 10_000_000).to_string());
    assert_eq!(row["segment_id"], (BASE + 1).to_string());
}

#[test]
fn paged_snapshot_resolves_older_layouts_and_keeps_the_same_sample_on_continuation() {
    let at = BASE + 20_000_000;
    let data = fixture(&[
        (
            BASE,
            &[
                Sample::LegacyActivity(at),
                Sample::LegacyActivityPid(at, 43),
            ],
        ),
        (
            BASE + 1,
            &[
                Sample::Host(BASE),
                Sample::Activity(BASE + 10_000_000),
                Sample::Activity(BASE + 30_000_000),
            ],
        ),
    ]);
    let mut query = snapshot_request(BASE + 1, at);
    let unpaged = snapshot(&data.context, query.clone());
    let rows = unpaged
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    let timestamp = at.to_string();
    assert!(rows.iter().all(|row| row["timestamp"] == timestamp));
    query.page_size = Some(1);
    for pid in [42, 43] {
        let records = snapshot(&data.context, query.clone());
        let rows = records
            .iter()
            .filter(|record| record["record"] == "row")
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["timestamp"], at.to_string());
        assert_eq!(rows[0]["segment_id"], BASE.to_string());
        assert_eq!(rows[0]["values"], json!([pid]));
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("page");
        query.cursor = page["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(query.cursor.is_some(), pid == 42);
    }
}

#[test]
fn exact_row_snapshot_keeps_its_source_when_an_earlier_segment_has_the_same_sample() {
    let at = BASE + 20_000_000;
    let data = fixture(&[
        (BASE, &[Sample::Activity(at), Sample::LegacyActivity(at)]),
        (BASE + 1, &[Sample::Activity(at)]),
    ]);
    let mut query = snapshot_request(BASE + 1, at);
    query.type_id = Some(PgStatActivityV3::CONTRACT.type_id.get());
    query.row_ordinal = Some(0);
    let records = snapshot(&data.context, query);
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["segment_id"], (BASE + 1).to_string());
    assert_eq!(rows[0]["timestamp"], at.to_string());
}

#[test]
fn snapshot_skips_resolved_layouts_while_searching_older_revisions() {
    let data = fixture(&[
        (
            BASE,
            &[
                Sample::LegacyActivity(BASE + 10),
                Sample::LegacyActivity(BASE + 11),
            ],
        ),
        (
            BASE + 1,
            &[Sample::Activity(BASE + 20), Sample::Activity(BASE + 21)],
        ),
        (BASE + 2, &[Sample::Activity(BASE + 90)]),
        (BASE + 3, &[Sample::Activity(BASE + 100)]),
        (
            BASE + 4,
            &[Sample::Activity(BASE + 90), Sample::Activity(BASE + 100)],
        ),
    ]);
    for latest in [false, true] {
        crate::snapshot::take_contributing_moment_rows();
        let mut query = snapshot_request(BASE + 4, BASE + 100);
        query.latest = latest;
        let records = snapshot(&data.context, query);
        let rows = records
            .iter()
            .filter(|record| record["record"] == "row")
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2, "equal-time contributors stay visible");
        let timestamp = (BASE + 100).to_string();
        assert!(rows.iter().all(|row| row["timestamp"] == timestamp));
        assert_eq!(
            crate::snapshot::take_contributing_moment_rows(),
            if latest { 6 } else { 4 },
            "the settled layout skips old rows while its older revision is still discovered"
        );
    }
}

#[test]
fn latest_snapshot_resolves_sections_independently_in_request_order() {
    let at = BASE + 20_000_000;
    let data = fixture(&[
        (BASE, &[Sample::Activity(at)]),
        (BASE + 1, &[Sample::Host(BASE)]),
    ]);
    let mut query = snapshot_request(BASE + 1, at);
    query.sections.push("os_loadavg".to_owned());
    query.fields.clear();
    query.by.clear();
    let records = snapshot(&data.context, query);
    let layouts = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| record["layout"]["logical_name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(layouts, [json!("pg_stat_activity"), json!("os_loadavg")]);
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|record| record["timestamp"].clone())
        .collect::<Vec<_>>();
    assert_eq!(rows, [json!(at.to_string()), json!(BASE.to_string())]);
}

#[test]
fn latest_snapshot_filters_the_selected_sample_without_falling_back_to_an_older_layout() {
    let at = BASE + 20_000_000;
    for (first, second) in [
        (
            Sample::Activity(BASE + 10_000_000),
            Sample::LegacyActivity(at),
        ),
        (
            Sample::LegacyActivity(at),
            Sample::Activity(BASE + 10_000_000),
        ),
    ] {
        let data = fixture(&[(BASE, &[first]), (BASE + 1, &[second])]);
        let mut query = snapshot_request(BASE + 1, at);
        query.filters.push(crate::Filter {
            column: "query_id".to_owned(),
            value: "42".to_owned(),
        });
        for page_size in [None, Some(1)] {
            query.page_size = page_size;
            let records = snapshot(&data.context, query.clone());
            assert!(records.iter().all(|record| record["record"] != "row"));
        }
    }
}

#[test]
fn sparse_activity_navigation_skips_subsecond_updates_and_other_sources() {
    let data = fixture(&[(
        BASE,
        &[
            Sample::Activity(BASE),
            Sample::Activity(BASE + 13_000),
            Sample::Activity(BASE + 999_999),
            Sample::Activity(BASE + 1_000_000),
            Sample::Host(BASE + 2_000_000),
            Sample::Activity(BASE + 17_000_000),
        ],
    )]);
    for (at, direction, expected) in [
        (BASE, Next, BASE + 1_000_000),
        (BASE + 1_000_000, Next, BASE + 17_000_000),
        (BASE + 17_000_000, Previous, BASE + 1_000_000),
        (BASE + 1_000_000, Previous, BASE),
    ] {
        assert_eq!(
            neighbor(&data.context, request(at, direction)),
            found(expected, BASE)
        );
    }
}

#[test]
fn overlapping_segments_do_not_hide_a_closer_neighbor_and_ties_use_newest_id() {
    let data = fixture(&[
        (
            BASE,
            &[Sample::Host(BASE), Sample::Activity(BASE + 9_000_000)],
        ),
        (
            BASE + 1,
            &[
                Sample::Host(BASE + 500_000),
                Sample::Activity(BASE + 3_000_000),
            ],
        ),
        (
            BASE + 2,
            &[
                Sample::Activity(BASE + 3_000_000),
                Sample::Host(BASE + 10_000_000),
            ],
        ),
    ]);
    assert_eq!(
        neighbor(&data.context, request(BASE, Next)),
        found(BASE + 3_000_000, BASE + 2)
    );
    assert_eq!(
        neighbor(&data.context, request(BASE + 11_000_000, Previous)),
        found(BASE + 9_000_000, BASE)
    );
    assert_eq!(
        neighbor(&data.context, request(BASE + 5_000_000, Previous)),
        found(BASE + 3_000_000, BASE + 2)
    );
}

#[test]
fn neighbor_crosses_hour_boundaries_and_physical_layout_versions() {
    let data = fixture(&[
        (BASE, &[Sample::LegacyActivity(BASE + 3_599_000_000)]),
        (
            BASE + 3_600_000_000,
            &[Sample::Activity(BASE + 3_601_000_000)],
        ),
    ]);
    assert_eq!(
        neighbor(&data.context, request(BASE + 3_600_000_000, Previous)),
        found(BASE + 3_599_000_000, BASE)
    );
    assert_eq!(
        neighbor(&data.context, request(BASE + 3_600_000_000, Next)),
        found(BASE + 3_601_000_000, BASE + 3_600_000_000)
    );
}

#[test]
fn multiple_sections_and_inclusive_bounds_select_only_visible_samples() {
    let data = fixture(&[(
        BASE,
        &[
            Sample::Activity(BASE + 9_000_000),
            Sample::Host(BASE + 2_000_000),
        ],
    )]);
    let mut query = request(BASE, Next);
    query.sections.push("os_loadavg".to_owned());
    query.window = Window {
        from: Some(BASE + 2_000_000),
        to: Some(BASE + 2_000_000),
    };
    assert_eq!(
        neighbor(&data.context, query.clone()),
        found(BASE + 2_000_000, BASE)
    );
    query.window.to = Some(BASE + 1_999_999);
    query.window.from = None;
    assert_eq!(neighbor(&data.context, query), absent());
}

#[test]
fn missing_samples_and_arithmetic_edges_return_uncacheable_null() {
    let data = fixture(&[(BASE, &[Sample::Host(BASE)])]);
    for (at, direction) in [(BASE, Next), (i64::MAX, Next), (i64::MIN, Previous)] {
        assert_eq!(neighbor(&data.context, request(at, direction)), absent());
    }
    let empty = fixture(&[]);
    assert_eq!(neighbor(&empty.context, request(BASE, Next)), absent());
}

#[test]
fn cancelled_neighbor_emits_no_result() {
    let data = fixture(&[(BASE, &[Sample::Activity(BASE + 2_000_000)])]);
    let execution = execute(
        &data.context,
        QueryRequest::SnapshotNeighbor(request(BASE, Next)),
    )
    .expect("prepare");
    let mut records = Records {
        cancelled: true,
        ..Records::default()
    };
    assert!(matches!(
        execution.stream(&mut records),
        Err(QueryError::Cancelled)
    ));
    assert!(records.rows.is_empty());
}

#[test]
fn typed_neighbor_requests_reject_invalid_section_sets_and_bounds() {
    let data = fixture(&[]);
    let mut query = request(BASE, Next);
    for sections in [
        vec![],
        vec![String::new()],
        vec!["x".repeat(129)],
        vec!["pg_stat_activity".to_owned(); 2],
        (0..33).map(|index| format!("section{index}")).collect(),
    ] {
        query.sections = sections;
        let error = execute(&data.context, QueryRequest::SnapshotNeighbor(query.clone()))
            .expect_err("invalid sections");
        assert_eq!(error.parameter(), Some("section"));
    }
    query = request(BASE, Previous);
    query.window = Window {
        from: Some(20),
        to: Some(10),
    };
    let error =
        execute(&data.context, QueryRequest::SnapshotNeighbor(query)).expect_err("reversed bounds");
    assert_eq!(error.parameter(), Some("to"));
}

#[derive(Debug)]
struct ObservedDataset {
    inner: Arc<dyn QueryDataset>,
    opened: Arc<Mutex<Vec<i64>>>,
}

impl QueryDataset for ObservedDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        self.inner.catalog()
    }

    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError> {
        self.inner.segment(id)
    }

    fn open(&self, segment: &DatasetSegment) -> Result<kronika_reader::Segment, QueryError> {
        self.opened
            .lock()
            .map_err(|_error| {
                QueryError::Unreadable(std::io::Error::other("neighbor test lock poisoned").into())
            })?
            .push(segment.id());
        self.inner.open(segment)
    }

    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        self.inner.at_active_position(segment, position)
    }
}

fn observed(context: &QueryContext) -> (QueryContext, Arc<Mutex<Vec<i64>>>) {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let dataset = Arc::new(ObservedDataset {
        inner: Arc::clone(&context.dataset),
        opened: Arc::clone(&opened),
    });
    (QueryContext::new(dataset, 0, false), opened)
}

#[test]
fn lock_cutoff_reads_the_newest_activity_first_and_stops_at_its_zero() {
    let data = fixture(&[
        (BASE, &[Sample::Graph(BASE + 100_000)]),
        (BASE + 1, &[Sample::LockWait(BASE + 1_000_000)]),
        (BASE + 2, &[Sample::Activity(BASE + 2_000_000)]),
        (BASE + 3, &[Sample::Activity(BASE + 3_000_000)]),
        (BASE + 4, &[Sample::LockWait(BASE + 4_000_000)]),
    ]);
    let locks = |segment_id: i64, at: i64| {
        let (context, opened) = observed(&data.context);
        let mut request = snapshot_request(segment_id, at);
        request.sections = vec!["pg_locks".to_owned()];
        let rows = snapshot(&context, request)
            .into_iter()
            .filter(|record| record["record"] == "row")
            .count();
        let opened = opened.lock().expect("opened lock").clone();
        (rows, opened)
    };
    // Only waits since the graph: it stays on screen.
    let (rows, opened) = locks(BASE + 1, BASE + 1_500_000);
    assert_eq!(rows, 1);
    assert!(opened.contains(&(BASE + 1)));
    // The newest segment shows no wait, so older activity is never opened.
    let (rows, opened) = locks(BASE + 3, BASE + 3_500_000);
    assert_eq!(rows, 0);
    assert!(opened.contains(&(BASE + 3)));
    assert!(!opened.contains(&(BASE + 2)) && !opened.contains(&(BASE + 1)));
    // A waiting newest segment continues to the next older one and stops there.
    let (rows, opened) = locks(BASE + 4, BASE + 4_500_000);
    assert_eq!(rows, 0);
    assert!(opened.contains(&(BASE + 4)) && opened.contains(&(BASE + 3)));
    assert!(!opened.contains(&(BASE + 2)) && !opened.contains(&(BASE + 1)));
}

#[test]
fn neighbor_prunes_unrelated_sections_and_stops_only_after_proven_bounds() {
    let data = fixture(&[
        (BASE, &[Sample::Activity(BASE + 500_000)]),
        (BASE + 1, &[Sample::Host(BASE + 1_000_000)]),
        (BASE + 2, &[Sample::Activity(BASE + 2_000_000)]),
        (BASE + 3, &[Sample::Activity(BASE + 3_000_000)]),
    ]);
    let (context, opened) = observed(&data.context);
    let execution = execute(
        &context,
        QueryRequest::SnapshotNeighbor(request(BASE, Next)),
    )
    .expect("prepare");
    assert!(
        opened.lock().expect("opened lock").is_empty(),
        "timestamp reads wait for the cancellable stream"
    );
    let mut records = Records::default();
    execution.stream(&mut records).expect("stream");
    assert_eq!(records.rows, [found(BASE + 2_000_000, BASE + 2)]);
    assert_eq!(*opened.lock().expect("opened lock"), [BASE + 2]);
}

#[test]
fn cancellation_during_timestamp_scan_does_not_emit_a_partial_neighbor() {
    let data = fixture(&[(
        BASE,
        &[
            Sample::Activity(BASE + 2_000_000),
            Sample::Activity(BASE + 3_000_000),
        ],
    )]);
    let (context, opened) = observed(&data.context);
    let execution = execute(
        &context,
        QueryRequest::SnapshotNeighbor(request(BASE, Next)),
    )
    .expect("prepare");
    let mut records = Records {
        cancel_after: Some(5),
        ..Records::default()
    };
    assert!(matches!(
        execution.stream(&mut records),
        Err(QueryError::Cancelled)
    ));
    assert_eq!(*opened.lock().expect("opened lock"), [BASE]);
    assert!(records.rows.is_empty());
}
