//! Storage-neutral execution of recorded-data queries.

#[cfg(test)]
use kronika_writer as _;

mod catalog;
mod dataset;
mod error;
mod events;
mod exact_product;
mod finished_dataset;
mod heatmap;
mod history;
mod hour;
mod index;
mod index_provider;
mod projection;
mod render;
mod request;
mod row_detail;
mod row_key;
mod rows;
mod selection;
pub mod snapshot;
mod statement_scope;
mod time;

pub use catalog::{CatalogFacts, CatalogField, CatalogSection, catalog_facts};
pub use dataset::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    OpaqueCapture, PredecessorSelection, QueryDataset, SegmentBounds, SegmentSelection,
};
pub use error::QueryError;
pub use events::{
    EventGroup, EventOccurrence, EventsQuery, EventsQueryError, EventsRepresentation, EventsResult,
    MAX_EVENTS_LIMIT, MAX_EVENTS_WINDOW_MICROS, execute_events, label_event_fields,
};
pub use finished_dataset::FinishedDataset;
pub use heatmap::{
    CoverageState, DEFAULT_TOP, HeatmapBand, HeatmapBatchQuery, HeatmapBatchResult,
    HeatmapCoverage, HeatmapEntity, HeatmapError, HeatmapGrid, HeatmapGroup, HeatmapInterval,
    HeatmapItemQuery, HeatmapItemResult, HeatmapSummary, HeatmapView, MAX_FIELDS, MAX_TOP,
    NamedValues, NormalizedRanking, ValidatedHeatmapQuery, execute_heatmap_batch,
    validate_heatmap_request,
};
pub use hour::{
    GroupKey, Metric, RelationAggregate, RelationField, RelationKind, RelationSource,
    index_scan_rate_is_zero, key_fields, output_fields,
};
pub use index_provider::{IndexProvider, IndexResource, MemoryIndexProvider};
pub use projection::{OutputField, Plan, plans, resolved_dictionary};
pub use request::{
    ActiveCursor, CatalogRequest, DataRequest, Filter, HourPart, HourRequest, HourSeriesRequest,
    IndexRequest, Order, QueryRequest, RelationGroup, RowsRequest, SegmentRequest, SnapshotRequest,
    Window,
};
pub use row_detail::{
    PreparedRowDetail, RowDetailResult, ValidatedRowDetailQuery, execute_row_detail,
    prepare_row_detail, validate_row_detail_ref,
};
pub use row_key::{
    DETAIL_REF_MAX_ENCODED_BYTES, DetailLocator, RowIdentity, detail_locator, identity,
    identity_columns, is_detail_text, validate,
};
pub use statement_scope::{COLLECTOR_STATEMENT_PREFIX, STATEMENTS_SECTION, StatementScope};
pub use time::TimeRange;

use catalog::PreparedCatalog;
use events::PreparedEvents;
use history::PreparedHistory;
use hour::PreparedHour;
use rows::PreparedRows;

/// Source-family bit for recorded operating-system data.
pub const SOURCE_OS: u32 = 1 << 0;
/// Source-family bit for recorded `PostgreSQL` data.
pub const SOURCE_POSTGRESQL: u32 = 1 << 1;

/// Return the source-family bit encoded by a registered section type ID.
#[must_use]
pub const fn source_bit(type_id: u32) -> Option<u32> {
    match type_id {
        1_001_001..=1_019_999 | 1_020_001 | 2_001_001..=2_199_999 => Some(SOURCE_POSTGRESQL),
        1_100_001..=1_299_999 => Some(SOURCE_OS),
        _ => None,
    }
}

/// Inputs shared by one synchronous query execution.
#[derive(Clone)]
pub struct QueryContext {
    dataset: std::sync::Arc<dyn QueryDataset>,
    indexes: Option<std::sync::Arc<dyn IndexProvider>>,
    configured_sources: u32,
    synthetic_demo: bool,
}

impl std::fmt::Debug for QueryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryContext")
            .field("dataset", &self.dataset)
            .field("indexes", &self.indexes)
            .field("configured_sources", &self.configured_sources)
            .field("synthetic_demo", &self.synthetic_demo)
            .finish()
    }
}

impl QueryContext {
    /// Bind a captured-data adapter and deployment facts to query execution.
    #[must_use]
    pub const fn new(
        dataset: std::sync::Arc<dyn QueryDataset>,
        configured_sources: u32,
        synthetic_demo: bool,
    ) -> Self {
        Self {
            dataset,
            indexes: None,
            configured_sources,
            synthetic_demo,
        }
    }

    /// Add the derived-index source used by indexed queries.
    #[must_use]
    pub fn with_index_provider(mut self, indexes: std::sync::Arc<dyn IndexProvider>) -> Self {
        self.indexes = Some(indexes);
        self
    }

    fn index_provider(&self) -> Result<&dyn IndexProvider, QueryError> {
        self.indexes.as_deref().ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "this query context has no derived-index provider",
            )))
        })
    }
}

/// Storage stability relevant to the native response adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStability {
    /// The captured data may change between requests.
    Mutable,
    /// The data is settled, but no reusable identity was derived.
    Revalidate,
    /// The response is bound to immutable captured data.
    Immutable,
}

/// Stable response identity whose wire representation belongs to the adapter.
#[derive(Debug, Clone, Copy)]
pub enum QueryIdentity<'a> {
    /// CRC32C validated from one immutable IDX container.
    IndexChecksum(u32),
    /// Exact immutable segment selection whose digest remains adapter-owned.
    SegmentSet {
        /// Stable response-family domain.
        resource: &'a str,
        /// Exact semantic request shape.
        shape: &'a str,
        /// Captured segment descriptors in selection order.
        segments: &'a [DatasetSegment],
    },
}

/// Transport-neutral facts known before records are emitted.
#[derive(Debug, Clone, Copy)]
pub struct QueryMetadata<'a> {
    stability: QueryStability,
    identity: Option<QueryIdentity<'a>>,
}

impl<'a> QueryMetadata<'a> {
    /// Stability of the selected recorded data.
    #[must_use]
    pub const fn stability(self) -> QueryStability {
        self.stability
    }

    /// Stable selected-data identity, when one is available.
    #[must_use]
    pub const fn identity(self) -> Option<QueryIdentity<'a>> {
        self.identity
    }

    const fn revalidate() -> Self {
        Self {
            stability: QueryStability::Revalidate,
            identity: None,
        }
    }
}

/// Receiver for already framed NDJSON records.
pub trait QuerySink {
    /// Accept one complete record including its trailing newline.
    fn record(&mut self, bytes: Vec<u8>) -> bool;

    /// Whether the caller no longer wants more work or output.
    fn cancelled(&self) -> bool;
}

/// One prepared execution with metadata available before streaming.
pub struct QueryExecution {
    prepared: Prepared,
}

enum Prepared {
    Catalog(PreparedCatalog),
    Heatmap(heatmap::PreparedHeatmap),
    Index(index::PreparedIndex),
    History(PreparedHistory),
    Hour(PreparedHour),
    Snapshot(snapshot::PreparedSnapshot),
    Rows(PreparedRows),
    Events(PreparedEvents),
    RowDetail(PreparedRowDetail),
}

impl std::fmt::Debug for QueryExecution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryExecution").finish_non_exhaustive()
    }
}

impl QueryExecution {
    /// Facts the native adapter maps to cache and validator policy.
    #[must_use]
    pub fn metadata(&self) -> QueryMetadata<'_> {
        match &self.prepared {
            Prepared::Catalog(_) => QueryMetadata::revalidate(),
            Prepared::Heatmap(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: prepared
                    .validator_input()
                    .map(|(resource, shape, segments)| QueryIdentity::SegmentSet {
                        resource,
                        shape,
                        segments,
                    }),
            },
            Prepared::Index(prepared) => QueryMetadata {
                stability: match prepared.kind() {
                    kronika_reader::SegmentKind::Finished => QueryStability::Immutable,
                    kronika_reader::SegmentKind::Active => QueryStability::Mutable,
                },
                identity: prepared.checksum().map(QueryIdentity::IndexChecksum),
            },
            Prepared::History(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: None,
            },
            Prepared::Hour(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: prepared
                    .validator_input()
                    .map(|(resource, shape, segments)| QueryIdentity::SegmentSet {
                        resource,
                        shape,
                        segments,
                    }),
            },
            Prepared::Snapshot(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: prepared
                    .validator_input()
                    .map(|(resource, shape, segments)| QueryIdentity::SegmentSet {
                        resource,
                        shape,
                        segments,
                    }),
            },
            Prepared::Rows(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: None,
            },
            Prepared::Events(prepared) => QueryMetadata {
                stability: prepared.stability(),
                identity: prepared
                    .validator_input()
                    .map(|(resource, shape, segments)| QueryIdentity::SegmentSet {
                        resource,
                        shape,
                        segments,
                    }),
            },
            Prepared::RowDetail(_) => QueryMetadata {
                stability: QueryStability::Mutable,
                identity: None,
            },
        }
    }

    /// Emit records until complete or the sink disconnects.
    ///
    /// # Errors
    ///
    /// Returns a semantic, decoding, or captured-source error.
    pub fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        match self.prepared {
            Prepared::Catalog(prepared) => prepared.stream(sink),
            Prepared::Heatmap(prepared) => prepared.stream(sink),
            Prepared::Index(prepared) => prepared.stream(sink),
            Prepared::History(prepared) => prepared.stream(sink),
            Prepared::Hour(prepared) => prepared.stream(sink),
            Prepared::Snapshot(prepared) => prepared.stream(sink),
            Prepared::Rows(prepared) => prepared.stream(sink),
            Prepared::Events(prepared) => prepared.stream(sink),
            Prepared::RowDetail(prepared) => prepared.stream(sink),
        }
    }
}

/// Validate and prepare one recorded-data query.
///
/// # Errors
///
/// Returns a semantic, decoding, or captured-source error.
pub fn execute(
    context: &QueryContext,
    request: QueryRequest,
) -> Result<QueryExecution, QueryError> {
    if let QueryRequest::Snapshot(request) = request {
        return snapshot::prepare_snapshot(context, request)?.finish();
    }
    let prepared = match request {
        QueryRequest::Catalog(request) => Prepared::Catalog(PreparedCatalog::prepare(
            context.dataset.as_ref(),
            request,
            context.configured_sources,
            context.synthetic_demo,
        )?),
        QueryRequest::Heatmap(request) => Prepared::Heatmap(heatmap::prepare(
            std::sync::Arc::clone(&context.dataset),
            request,
        )?),
        QueryRequest::Index(request) => Prepared::Index(index::prepare(
            context.dataset.as_ref(),
            context.index_provider()?,
            &request,
        )?),
        QueryRequest::History(request) => {
            Prepared::History(history::prepare(context.dataset.as_ref(), request)?)
        }
        QueryRequest::Hour(request) => Prepared::Hour(hour::prepare(
            std::sync::Arc::clone(&context.dataset),
            context.indexes.clone(),
            request,
            context.configured_sources,
            context.synthetic_demo,
        )?),
        QueryRequest::Snapshot(_) => unreachable!("snapshot handled before shared preparation"),
        QueryRequest::Rows(request) => {
            Prepared::Rows(rows::prepare(context.dataset.as_ref(), request)?)
        }
        QueryRequest::Events(request) => Prepared::Events(events::prepare(
            std::sync::Arc::clone(&context.dataset),
            request,
        )?),
        QueryRequest::RowDetail(request) => {
            Prepared::RowDetail(prepare_row_detail(context, request)?)
        }
    };
    Ok(QueryExecution { prepared })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
