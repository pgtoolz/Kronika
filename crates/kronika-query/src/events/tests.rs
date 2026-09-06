use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::Segment;
use kronika_registry::pg_log::{PgLogErrors, PgLogTempFiles};
use kronika_registry::{StrId, Ts};
use kronika_store::PosixSource;
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Map, Value, json};

use super::{
    EventDataRow, EventSource, EventStat, EventsQuery, EventsRepresentation, EventsResult,
    OccurrenceAccumulator, execute_events, group::EventGroups,
};
use crate::{
    CapturedCatalog, DatasetListing, DatasetSegment, FinishedDataset, QueryContext, QueryDataset,
    QueryError, QuerySink, SegmentBounds, SegmentSelection, TimeRange,
};

const SEGMENT_ID: i64 = 1_780_000_000_000_000;

struct FinishedFixture {
    _directory: tempfile::TempDir,
    context: QueryContext,
}

fn intern(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("fixture string fits")
            .get(),
    )
}

fn finished_fixture(errors: &[(i64, &str)], temp_files: &[(i64, i64)]) -> FinishedFixture {
    let directory = tempfile::tempdir().expect("temporary event root");
    let root = DataRoot::open(directory.path()).expect("open event data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire event writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open event journal");
    let segment_id = SegmentId::new(SEGMENT_ID).expect("event segment id");
    let address = SegmentAddress::new(segment_id).expect("event segment address");

    let mut interner = Interner::new(DictLimits::default());
    let source_file = intern(&mut interner, "postgresql.log");
    let mut buffers = SectionBuffers::new();
    for &(at, pattern) in errors {
        let pattern = intern(&mut interner, pattern);
        buffers
            .push(PgLogErrors {
                ts: Ts(at),
                system_identifier: Some(42),
                source_file,
                severity: 0,
                category: 8,
                sqlstate: None,
                pattern,
                count: 1,
                sample: pattern,
                detail: None,
                hint: None,
                context: None,
                statement: None,
                database: None,
                username: None,
            })
            .expect("event error row fits");
    }
    for &(at, size_bytes) in temp_files {
        buffers
            .push(PgLogTempFiles {
                ts: Ts(at),
                system_identifier: Some(42),
                source_file,
                path: None,
                size_bytes,
                statement: None,
            })
            .expect("temporary-file row fits");
    }
    let dictionary = dict::encode(interner.window()).expect("encode event dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode event rows")
        .expect("nonempty event rows");
    journal
        .append(segment_id, &part)
        .expect("append event rows");
    write_segment(&journal, &owner, address).expect("publish event segment");
    drop(journal);
    drop(owner);

    let source = PosixSource::open(directory.path()).expect("open finished event source");
    FinishedFixture {
        _directory: directory,
        context: QueryContext::new(Arc::new(FinishedDataset::new(source)), 0, false),
    }
}

fn object(value: Value) -> Map<String, Value> {
    let Value::Object(value) = value else {
        panic!("fixture value must be an object");
    };
    value
}

fn retained_row(ordinal: u64, timestamp: i64) -> EventDataRow {
    let values = object(json!({ "sequence": ordinal }));
    EventDataRow {
        segment_id: 7,
        type_id: 2_001_001,
        row_ordinal: ordinal,
        timestamp,
        identity: values.clone(),
        values,
    }
}

fn pgbouncer_row(ordinal: u64, timestamp: i64, host: &str) -> EventDataRow {
    let values = object(json!({
        "source_file": "/var/log/pgbouncer.log",
        "level": 3,
        "database": "(nodb)",
        "username": "(nouser)",
        "host": host,
        "text": "no such database: nope",
    }));
    EventDataRow {
        segment_id: 7,
        type_id: 2_100_001,
        row_ordinal: ordinal,
        timestamp,
        identity: values.clone(),
        values,
    }
}

#[derive(Debug)]
struct EmptyDataset;

#[derive(Debug)]
struct EmptyCatalog;

impl CapturedCatalog for EmptyCatalog {
    fn ranges(&self) -> &[(i64, i64)] {
        &[]
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        assert_eq!(
            selection,
            SegmentSelection::new(SegmentBounds::half_open(10, 20))
        );
        Ok(DatasetListing {
            segments: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

impl QueryDataset for EmptyDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        Ok(Box::new(EmptyCatalog))
    }

    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("event range query does not select one segment")
    }

    fn open(&self, _segment: &DatasetSegment) -> Result<Segment, QueryError> {
        unreachable!("empty event query does not open a segment")
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        unreachable!("empty event query does not pin active data")
    }
}

struct Control(bool);

impl QuerySink for Control {
    fn record(&mut self, _bytes: Vec<u8>) -> bool {
        false
    }

    fn cancelled(&self) -> bool {
        self.0
    }
}

#[test]
fn typed_events_execution_returns_the_result_and_observes_cancellation() {
    let context = QueryContext::new(Arc::new(EmptyDataset), 0, false);
    let query = EventsQuery::normalize(
        TimeRange::new(10, 20).expect("valid event range"),
        Some(vec!["pg_log_errors".to_owned()]),
        EventsRepresentation::Occurrences,
        2,
    )
    .expect("valid event query");

    assert_eq!(
        execute_events(&context, query.clone(), &Control(false)).expect("typed event result"),
        EventsResult::Occurrences {
            occurrences: Vec::new(),
            truncated: false,
        }
    );
    assert!(matches!(
        execute_events(&context, query, &Control(true)),
        Err(QueryError::Cancelled)
    ));
}

#[test]
fn pgbouncer_group_uses_the_message_title_and_shared_connection_context() {
    let mut groups = EventGroups::new(SEGMENT_ID);
    groups.observe(
        EventSource::Pgbouncer,
        pgbouncer_row(1, SEGMENT_ID + 10, "10.0.0.7"),
    );
    groups.observe(
        EventSource::Pgbouncer,
        pgbouncer_row(2, SEGMENT_ID + 20, "10.0.0.7"),
    );
    let groups = groups.finish(None).expect("PgBouncer groups");

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].label.as_deref(), Some("no such database: nope"));
    assert_eq!(groups[0].count.to_bits(), 2.0_f64.to_bits());
    assert_eq!(
        groups[0].stat,
        EventStat::Pgbouncer {
            level: 3.0,
            database: Some("(nodb)".to_owned()),
            username: Some("(nouser)".to_owned()),
            host: Some("10.0.0.7".to_owned()),
            source_file: Some("/var/log/pgbouncer.log".to_owned()),
        }
    );

    let mut mixed = EventGroups::new(SEGMENT_ID);
    mixed.observe(
        EventSource::Pgbouncer,
        pgbouncer_row(1, SEGMENT_ID + 10, "10.0.0.7"),
    );
    mixed.observe(
        EventSource::Pgbouncer,
        pgbouncer_row(2, SEGMENT_ID + 20, "10.0.0.8"),
    );
    let mixed = mixed.finish(None).expect("mixed PgBouncer group");
    let EventStat::Pgbouncer { host, .. } = &mixed[0].stat else {
        panic!("PgBouncer stat")
    };
    assert_eq!(host, &None);
}

#[test]
fn occurrence_retention_is_limit_plus_one_and_keeps_semantic_order() {
    let query = EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 1_000_000).expect("valid event range"),
        Some(vec![
            "pg_log_errors".to_owned(),
            "pg_log_temp_files".to_owned(),
        ]),
        EventsRepresentation::Occurrences,
        7,
    )
    .expect("valid event query");
    let mut accumulator = OccurrenceAccumulator::new(&query);
    let mut expected = Vec::new();
    let mut source_encounters = [0_u64; 2];
    let mut peak = 0;

    for ordinal in 0_u64..20_000 {
        let source_rank = usize::from(ordinal % 2 != 0);
        let encounter = source_encounters[source_rank];
        source_encounters[source_rank] += 1;
        let timestamp =
            SEGMENT_ID + i64::try_from((20_000 - ordinal) % 113).expect("small timestamp offset");
        expected.push((timestamp, source_rank, encounter, ordinal));
        accumulator.observe(
            source_rank,
            query.sources[source_rank],
            retained_row(ordinal, timestamp),
        );
        peak = peak.max(accumulator.rows.len());
    }

    expected.sort_by_key(|(timestamp, source_rank, encounter, _ordinal)| {
        (*timestamp, *source_rank, *encounter)
    });
    let expected = expected
        .into_iter()
        .take(query.limit)
        .map(|(_timestamp, _source_rank, _encounter, ordinal)| ordinal)
        .collect::<Vec<_>>();
    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = accumulator.finish()
    else {
        panic!("occurrence result");
    };

    assert!(truncated);
    assert_eq!(peak, query.limit + 1);
    assert_eq!(
        occurrences
            .iter()
            .map(|occurrence| occurrence.detail_locator.row_ordinal)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn typed_execution_orders_timestamp_then_requested_source_and_truncates() {
    let fixture = finished_fixture(
        &[(SEGMENT_ID + 10, "error-a"), (SEGMENT_ID + 20, "error-b")],
        &[(SEGMENT_ID + 10, 100), (SEGMENT_ID + 30, 300)],
    );
    let query = EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 100).expect("valid event range"),
        Some(vec![
            "pg_log_temp_files".to_owned(),
            "pg_log_errors".to_owned(),
        ]),
        EventsRepresentation::Occurrences,
        3,
    )
    .expect("valid event query");
    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = execute_events(&fixture.context, query, &Control(false)).expect("typed event result")
    else {
        panic!("occurrence result");
    };

    assert!(truncated);
    assert_eq!(
        occurrences
            .iter()
            .map(|occurrence| (occurrence.source.as_str(), occurrence.detail_locator.at,))
            .collect::<Vec<_>>(),
        [
            ("pg_log_temp_files", SEGMENT_ID + 10),
            ("pg_log_errors", SEGMENT_ID + 10),
            ("pg_log_errors", SEGMENT_ID + 20),
        ]
    );
}

#[test]
fn typed_execution_keeps_content_equivalent_event_occurrences() {
    let fixture = finished_fixture(
        &[
            (SEGMENT_ID + 10, "duplicate"),
            (SEGMENT_ID + 10, "duplicate"),
        ],
        &[],
    );
    let query = EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 100).expect("valid event range"),
        Some(vec!["pg_log_errors".to_owned()]),
        EventsRepresentation::Occurrences,
        2,
    )
    .expect("valid event query");

    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = execute_events(&fixture.context, query, &Control(false)).expect("duplicate events")
    else {
        panic!("occurrence result");
    };
    assert!(!truncated);
    assert_eq!(occurrences.len(), 2);
    assert_ne!(
        occurrences[0].detail_locator.row_ordinal,
        occurrences[1].detail_locator.row_ordinal,
    );
    assert_eq!(
        occurrences[0].detail_locator.identity,
        occurrences[1].detail_locator.identity,
    );

    let query = EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 100).expect("valid event range"),
        Some(vec!["pg_log_errors".to_owned()]),
        EventsRepresentation::Groups,
        2,
    )
    .expect("valid event query");
    let EventsResult::Groups { groups, truncated } =
        execute_events(&fixture.context, query, &Control(false)).expect("duplicate event group")
    else {
        panic!("group result");
    };
    assert!(!truncated);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].count.to_bits(), 2.0_f64.to_bits());
    assert_eq!(groups[0].representative_ts, SEGMENT_ID + 10);
}
