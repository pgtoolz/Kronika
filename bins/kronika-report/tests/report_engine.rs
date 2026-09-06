//! Internal query composition over one production-written standalone fixture.

use std::error::Error as _;
use std::sync::Arc;
#[cfg(feature = "generator")]
use {base64 as _, flate2 as _, kronika_format as _, kronika_reader as _, tempfile as _};

use kronika_index::IndexError;
use kronika_layout::{LayoutError, SegmentId};
use kronika_query::{
    CatalogRequest, DataRequest, EventsQuery, EventsRepresentation, FinishedDataset,
    HeatmapBatchQuery, HeatmapItemQuery, HeatmapView, HourPart, HourRequest, IndexRequest,
    MemoryIndexProvider, NormalizedRanking, Order, QueryContext, QueryDataset, QueryError,
    QueryRequest, QuerySink, RowsRequest, SOURCE_OS, SOURCE_POSTGRESQL, SegmentRequest,
    SnapshotRequest, StatementScope, TimeRange, Window, detail_locator, execute,
    validate_heatmap_request, validate_row_detail_ref,
};
use kronika_report::{ReportEngine, ReportError, ReportInput};
use kronika_store::{EmbeddedSource, ResourceError};
use serde_json::{Map, Value, json};

const SEGMENT_ID_VALUE: i64 = 1_709_164_800_000_000;
const SEGMENT_ID_TEXT: &str = "1709164800000000";
const SAMPLE_TO: i64 = SEGMENT_ID_VALUE + 1_000_000;
const PROCESS_TYPE_ID: u32 = 1_100_001;
const ZMS: &[u8] = include_bytes!("fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("fixtures/standalone.idx");

struct Records {
    bytes: Vec<u8>,
    calls: usize,
    cancelled: bool,
    accept: bool,
}

impl Records {
    const fn accepting() -> Self {
        Self {
            bytes: Vec::new(),
            calls: 0,
            cancelled: false,
            accept: true,
        }
    }
}

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.calls += 1;
        if self.accept {
            self.bytes.extend_from_slice(&bytes);
        }
        self.accept
    }

    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

fn segment_id() -> SegmentId {
    SegmentId::new(SEGMENT_ID_VALUE).expect("fixture segment identity")
}

fn report_input(segment_id: SegmentId) -> ReportInput {
    ReportInput {
        segment_id,
        zms: ZMS.to_vec(),
        idx: IDX.to_vec(),
        configured_sources: SOURCE_OS | SOURCE_POSTGRESQL,
        max_zms_bytes: u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    }
}

fn direct_context(segment_id: SegmentId) -> QueryContext {
    let source = EmbeddedSource::from_owned(
        segment_id,
        ZMS.to_vec(),
        u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    )
    .expect("fixture ZMS");
    let indexes = MemoryIndexProvider::new(segment_id, IDX.to_vec()).expect("fixture IDX");
    let dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(source));
    QueryContext::new(dataset, SOURCE_OS | SOURCE_POSTGRESQL, false)
        .with_index_provider(Arc::new(indexes))
}

fn snapshot_request(segment_id: SegmentId) -> QueryRequest {
    QueryRequest::Snapshot(SnapshotRequest {
        segment_id: segment_id.get(),
        at: SEGMENT_ID_VALUE,
        sections: vec!["os_process".to_owned()],
        fields: vec!["comm".to_owned(), "utime".to_owned()],
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
        scope: StatementScope::All,
    })
}

fn request_families(segment_id: SegmentId) -> Vec<(&'static str, &'static str, QueryRequest)> {
    let data = DataRequest {
        segment: SegmentRequest {
            segment_id: segment_id.get(),
            section: "os_process".to_owned(),
        },
        fields: vec!["comm".to_owned(), "utime".to_owned()],
        filters: Vec::new(),
        type_id: None,
        after: None,
    };
    let heatmap = validate_heatmap_request(HeatmapBatchQuery {
        range: TimeRange::new(SEGMENT_ID_VALUE, SAMPLE_TO + 1).expect("fixture range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_cpu".to_owned(),
                fields: vec!["user".to_owned()],
                top: 1,
            },
            view: HeatmapView::Grid {
                columns: 1,
                group: Vec::new(),
                type_id: None,
            },
            scope: StatementScope::All,
        }],
    })
    .expect("fixture heatmap request");
    let events = EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID_VALUE, SAMPLE_TO + 1).expect("fixture event range"),
        Some(vec!["pg_log_errors".to_owned()]),
        EventsRepresentation::Occurrences,
        5,
    )
    .expect("fixture events request");
    let detail_ref = detail_locator(
        "os_process",
        segment_id.get(),
        SAMPLE_TO,
        PROCESS_TYPE_ID,
        1,
        Map::from_iter([("pid".to_owned(), json!("41"))]),
    )
    .detail_ref()
    .expect("fixture detail reference");
    let row_detail = validate_row_detail_ref(&detail_ref).expect("validated detail reference");

    vec![
        (
            "catalog",
            "finished_segment",
            QueryRequest::Catalog(CatalogRequest::default()),
        ),
        ("heatmap", "heatmap_row", QueryRequest::Heatmap(heatmap)),
        (
            "index",
            "point",
            QueryRequest::Index(IndexRequest {
                segment_id: segment_id.get(),
                section: "pg_stat_database".to_owned(),
            }),
        ),
        ("history", "row", QueryRequest::History(data.clone())),
        (
            "hour",
            "point",
            QueryRequest::Hour(HourRequest {
                window: Window {
                    from: Some(SEGMENT_ID_VALUE),
                    to: Some(SAMPLE_TO),
                },
                series: None,
                part: HourPart::Base,
                segments: None,
                active: None,
            }),
        ),
        ("snapshot", "row", snapshot_request(segment_id)),
        (
            "rows",
            "row",
            QueryRequest::Rows(RowsRequest {
                data,
                order: Order::Asc,
                page_size: 10,
                cursor: None,
            }),
        ),
        ("events", "event_occurrence", QueryRequest::Events(events)),
        (
            "row_detail",
            "row_detail",
            QueryRequest::RowDetail(row_detail),
        ),
    ]
}

fn direct_bytes(context: &QueryContext, request: QueryRequest) -> Result<Vec<u8>, QueryError> {
    let execution = execute(context, request)?;
    let mut sink = Records::accepting();
    execution.stream(&mut sink)?;
    Ok(sink.bytes)
}

fn report_bytes(engine: &ReportEngine, request: QueryRequest) -> Result<Vec<u8>, QueryError> {
    let mut sink = Records::accepting();
    engine.execute(request, &mut sink)?;
    Ok(sink.bytes)
}

fn ndjson(bytes: &[u8]) -> Vec<Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("fixture NDJSON record"))
        .collect()
}

#[test]
fn report_error_preserves_a_typed_layout_failure() {
    let layout: ReportError = SegmentId::new(i64::MAX)
        .expect_err("invalid segment identity")
        .into();
    assert!(matches!(
        layout,
        ReportError::Layout(LayoutError::SegmentIdOutOfRange(i64::MAX))
    ));
    assert!(layout.source().is_some());
}

#[test]
fn construction_preserves_typed_resource_and_index_failures() {
    let mut over_limit = report_input(segment_id());
    over_limit.max_zms_bytes -= 1;
    let error = ReportEngine::new(over_limit).expect_err("logical limit must apply first");
    assert!(matches!(
        error,
        ReportError::Resource(ResourceError::TooLarge { len, max })
            if len == ZMS.len() as u64 && max == ZMS.len() as u64 - 1
    ));

    let mut bad_zms = report_input(segment_id());
    bad_zms.zms[0] ^= 0xff;
    let error = ReportEngine::new(bad_zms).expect_err("damaged ZMS must fail");
    assert!(matches!(
        error,
        ReportError::Resource(ResourceError::BadMagic)
    ));

    let mut bad_idx = report_input(segment_id());
    bad_idx.idx[0] ^= 0xff;
    let error = ReportEngine::new(bad_idx).expect_err("damaged IDX must fail");
    assert!(matches!(error, ReportError::Index(IndexError::BadMagic)));
    assert!(error.source().is_some());
}

#[test]
fn construction_bounds_logical_length_not_owned_capacity() {
    let mut zms = Vec::with_capacity(ZMS.len() + 4_097);
    zms.extend_from_slice(ZMS);
    let mut idx = Vec::with_capacity(IDX.len() + 257);
    idx.extend_from_slice(IDX);
    assert!(zms.capacity() > ZMS.len());
    assert!(idx.capacity() > IDX.len());

    ReportEngine::new(ReportInput {
        segment_id: segment_id(),
        zms,
        idx,
        configured_sources: SOURCE_OS | SOURCE_POSTGRESQL,
        max_zms_bytes: u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    })
    .expect("allocation capacity is not the logical ZMS length");
}

#[test]
fn explicit_segment_id_binds_both_embedded_artifacts() {
    let rebound = SegmentId::new(SEGMENT_ID_VALUE + 86_400_000_000)
        .expect("second explicit segment identity");
    let engine = ReportEngine::new(report_input(rebound)).expect("rebound report engine");
    let rebound_text = rebound.to_string();
    let catalog = ndjson(
        &report_bytes(&engine, QueryRequest::Catalog(CatalogRequest::default()))
            .expect("rebound catalog"),
    );
    assert!(catalog.iter().any(|record| {
        record["record"] == "finished_segment"
            && record["id"].as_str() == Some(rebound_text.as_str())
    }));

    let index = report_bytes(
        &engine,
        QueryRequest::Index(IndexRequest {
            segment_id: rebound.get(),
            section: "pg_stat_database".to_owned(),
        }),
    )
    .expect("IDX is bound to the explicit identity");
    assert!(!ndjson(&index).is_empty());

    let mut sink = Records::accepting();
    let error = engine
        .execute(
            QueryRequest::Index(IndexRequest {
                segment_id: SEGMENT_ID_VALUE,
                section: "health".to_owned(),
            }),
            &mut sink,
        )
        .expect_err("the fixture's generation-time ID is not implicit");
    assert!(matches!(error, QueryError::NoSuchSegment));
}

#[test]
fn report_engine_matches_direct_context_for_all_nine_query_families() {
    let segment_id = segment_id();
    let context = direct_context(segment_id);
    let engine = ReportEngine::new(report_input(segment_id)).expect("report engine");
    let requests = request_families(segment_id);
    assert_eq!(requests.len(), 9);

    for (family, expected_record, request) in requests {
        let direct = direct_bytes(&context, request.clone())
            .unwrap_or_else(|error| panic!("direct {family} failed: {error}"));
        let report = report_bytes(&engine, request)
            .unwrap_or_else(|error| panic!("report {family} failed: {error}"));
        assert_eq!(report, direct, "{family} bytes");
        let records = ndjson(&report);
        assert!(
            records
                .iter()
                .any(|record| record["record"] == expected_record),
            "{family} has no {expected_record} record"
        );

        if family == "events" {
            assert!(records.iter().any(|record| {
                record["record"] == "event_occurrence" && record["source"] == "pg_log_errors"
            }));
        }
        if family == "index" {
            assert!(records.iter().any(|record| {
                record["record"] == "point"
                    && record["series"] == "transactions_per_second"
                    && !record["value"].is_null()
            }));
        }
        if family == "snapshot" {
            assert!(
                records.iter().any(|record| {
                    record["record"] == "row"
                        && record["timestamp"].as_str() == Some(SEGMENT_ID_TEXT)
                        && record["values"][1].is_null()
                }),
                "the first standalone rate point remains unavailable"
            );
        }
    }
}

#[test]
fn cancellation_and_sink_refusal_stop_without_adapter_state() {
    let engine = ReportEngine::new(report_input(segment_id())).expect("report engine");
    let mut cancelled = Records {
        bytes: Vec::new(),
        calls: 0,
        cancelled: true,
        accept: true,
    };
    engine
        .execute(
            QueryRequest::Catalog(CatalogRequest::default()),
            &mut cancelled,
        )
        .expect("shared catalog cancellation");
    assert_eq!(cancelled.calls, 0);
    assert!(cancelled.bytes.is_empty());

    let mut refusing = Records {
        bytes: Vec::new(),
        calls: 0,
        cancelled: false,
        accept: false,
    };
    engine
        .execute(
            QueryRequest::Catalog(CatalogRequest::default()),
            &mut refusing,
        )
        .expect("shared sink refusal");
    assert_eq!(refusing.calls, 1);
    assert!(refusing.bytes.is_empty());
}
