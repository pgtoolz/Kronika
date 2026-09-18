//! Recorded-data request parsing and error status shared by HTTP and embedded reports.

mod heatmap;
mod hour;
mod parameters;
mod rows;
mod snapshot;

use heatmap::parse_heatmap;
use hour::{parse_catalog, parse_hour};
use kronika_query::{
    DETAIL_REF_MAX_ENCODED_BYTES, EventsQuery, EventsRepresentation, HeatmapBatchQuery,
    HeatmapItemQuery, HeatmapView, IndexRequest, MAX_EVENTS_LIMIT, MAX_EVENTS_WINDOW_MICROS,
    NormalizedRanking, QueryError, QueryRequest, SegmentRequest, StatementScope, TimeRange,
};
pub use parameters::pairs as query_pairs;
use parameters::{bounded, decoded, number, pairs};
use rows::{parse_data, parse_rows};
use snapshot::{parse_snapshot, parse_snapshot_neighbor};

// Physical rows returned when `page_size` is omitted from a rows request.
const DEFAULT_PAGE_SIZE: usize = 100;
// Caps the rows emitted by one physical-row page.
const MAX_PAGE_SIZE: usize = 1_000;
// Rows returned by a paged snapshot when `page_size` is omitted.
const DEFAULT_SNAPSHOT_PAGE_SIZE: usize = 200;
/// Maximum accepted encoded query-string length.
pub const MAX_QUERY_BYTES: usize = 64 * 1024;
// Maximum decoded section-name length in bytes.
const MAX_SECTION_BYTES: usize = 128;
// Caps the sections composed into one snapshot.
const MAX_SNAPSHOT_SECTIONS: usize = 16;
/// Maximum number of rows accepted by a snapshot page request.
pub const MAX_SNAPSHOT_PAGE_SIZE: usize = 5_000;
// Maximum decoded search length in Unicode scalar values, not UTF-8 bytes.
const MAX_SEARCH_EXPRESSION_CHARS: usize = 1_024;
// Caps projected columns in snapshots, hour series, history, and row pages.
const MAX_FIELDS: usize = 256;
// Time buckets used when `columns` is omitted; bucket width follows the range.
const DEFAULT_HEATMAP_COLUMNS: usize = 60;
// Caps output grid width; MAX_HEATMAP_TOP separately caps ranked entities.
const MAX_HEATMAP_COLUMNS: usize = 1_440;
// Caps the fields forming a heatmap aggregation key.
const MAX_HEATMAP_GROUP: usize = 4;
// Caps repeated `where.*` predicates in one request.
const MAX_FILTERS: usize = 64;
// Caps snapshot sort keys, in precedence order.
const MAX_ORDER_FIELDS: usize = 16;

/// Recorded-data resource requests accepted by native and embedded adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A query ready for execution.
    Query(QueryRequest),
    /// Heatmap arguments awaiting range and field validation.
    Heatmap(HeatmapRequest),
    /// An opaque row reference awaiting validation.
    RowDetail(String),
}

/// Parsed HTTP shape for one ranked heatmap resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeatmapRequest {
    from: i64,
    to: i64,
    section: String,
    fields: Vec<String>,
    columns: usize,
    top: usize,
    group: Vec<String>,
    type_id: Option<u32>,
    scope: StatementScope,
}

impl Route {
    /// Validate heatmap arguments or a row reference before opening storage.
    /// Other routes already contain their query.
    ///
    /// The HTTP adapter calls this after checking the method and response encoding.
    ///
    /// # Errors
    ///
    /// Returns the query error for invalid heatmap arguments or a malformed row reference.
    pub fn into_query(self) -> Result<QueryRequest, QueryError> {
        match self {
            Self::Query(request) => Ok(request),
            Self::Heatmap(request) => request.into_query().map(QueryRequest::Heatmap),
            Self::RowDetail(detail_ref) => {
                kronika_query::validate_row_detail_ref(&detail_ref).map(QueryRequest::RowDetail)
            }
        }
    }
}

impl HeatmapRequest {
    fn into_query(self) -> Result<kronika_query::ValidatedHeatmapQuery, QueryError> {
        let to_exclusive = self
            .to
            .checked_add(1)
            .ok_or_else(|| QueryError::BadFilter("to".to_owned()))?;
        let range = TimeRange::new(self.from, to_exclusive)
            .map_err(|_error| QueryError::BadFilter("to".to_owned()))?;
        kronika_query::validate_heatmap_request(HeatmapBatchQuery {
            range,
            items: vec![HeatmapItemQuery {
                ranking: NormalizedRanking {
                    section: self.section,
                    fields: self.fields,
                    top: self.top,
                },
                view: HeatmapView::Grid {
                    columns: self.columns,
                    group: self.group,
                    type_id: self.type_id,
                },
                scope: self.scope,
            }],
        })
    }
}

/// Why a request was refused before opening a dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// No resource has this path shape.
    NoSuchPath,
    /// One named path/query field is absent, duplicated, or invalid.
    BadParameter(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "no such path"),
            Self::BadParameter(name) => write!(f, "invalid parameter {name}"),
        }
    }
}

/// HTTP/Fetch status for a query failure, shared by native and embedded responses.
/// Transport-specific headers and response bodies belong to the caller.
#[must_use]
pub const fn query_error_status(error: &QueryError) -> u16 {
    match error {
        QueryError::NoSuchSegment | QueryError::NoSuchSection => 404,
        QueryError::NoSuchColumn(_)
        | QueryError::MixedUnits(_)
        | QueryError::BadFilter(_)
        | QueryError::BadCursor
        | QueryError::BadLocator(_) => 400,
        _ => 500,
    }
}

/// Parse a path and optional query into a typed resource request.
///
/// # Errors
///
/// Returns a path or named-parameter refusal; malformed percent escapes and
/// non-UTF-8 data are always refused.
pub fn parse(path: &str, query: Option<&str>) -> Result<Route, RouteError> {
    let query = query.unwrap_or("");
    if query.len() > MAX_QUERY_BYTES {
        return Err(RouteError::BadParameter("query".to_owned()));
    }
    if path == "/api/catalog" {
        return parse_catalog(query).map(|request| Route::Query(QueryRequest::Catalog(request)));
    }
    if path == "/api/hour" {
        return parse_hour(query).map(|request| Route::Query(QueryRequest::Hour(request)));
    }
    if path == "/api/heatmap" {
        return parse_heatmap(query).map(Route::Heatmap);
    }
    if path == "/api/events" {
        return parse_events(query).map(|request| Route::Query(QueryRequest::Events(request)));
    }
    if path == "/api/row-detail" {
        return parse_row_detail(query).map(Route::RowDetail);
    }
    if path == "/api/snapshot/neighbor" {
        return parse_snapshot_neighbor(query)
            .map(|request| Route::Query(QueryRequest::SnapshotNeighbor(request)));
    }
    let tail = path
        .strip_prefix("/api/segments/")
        .ok_or(RouteError::NoSuchPath)?;
    let pieces: Vec<&str> = tail.split('/').collect();
    if pieces.len() == 2 && pieces[1] == "snapshot" && !pieces[0].is_empty() {
        return parse_snapshot(number("segment_id", pieces[0])?, query)
            .map(|request| Route::Query(QueryRequest::Snapshot(request)));
    }
    if pieces.len() != 4 || pieces[1] != "sections" || pieces.iter().any(|piece| piece.is_empty()) {
        return Err(RouteError::NoSuchPath);
    }
    let segment_id = number("segment_id", pieces[0])?;
    let section = decoded("section", pieces[2], false)?;
    if section.is_empty() || section.len() > MAX_SECTION_BYTES {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    let segment = SegmentRequest {
        segment_id,
        section,
    };
    match pieces[3] {
        "index" if pairs(query)?.is_empty() => {
            Ok(Route::Query(QueryRequest::Index(IndexRequest {
                segment_id: segment.segment_id,
                section: segment.section,
            })))
        }
        "index" => Err(RouteError::BadParameter("query".to_owned())),
        "history" => {
            parse_data(segment, query).map(|request| Route::Query(QueryRequest::History(request)))
        }
        "rows" => {
            parse_rows(segment, query).map(|request| Route::Query(QueryRequest::Rows(request)))
        }
        _ => Err(RouteError::NoSuchPath),
    }
}

fn parse_events(query: &str) -> Result<EventsQuery, RouteError> {
    let mut from = None;
    let mut to = None;
    let mut representation = None;
    let mut limit = None;
    let mut sources = Vec::new();
    let mut saw_source = false;
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if from.is_none() => from = Some(number("from", &value)?),
            "to" if to.is_none() => to = Some(number("to", &value)?),
            "representation" if representation.is_none() => {
                representation = Some(match value.as_str() {
                    "groups" => EventsRepresentation::Groups,
                    "occurrences" => EventsRepresentation::Occurrences,
                    _ => return Err(RouteError::BadParameter("representation".to_owned())),
                });
            }
            "limit" if limit.is_none() => limit = Some(bounded("limit", &value, MAX_EVENTS_LIMIT)?),
            "source" => {
                saw_source = true;
                sources.push(value);
            }
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    let range = TimeRange::bounded(from, to, MAX_EVENTS_WINDOW_MICROS)
        .map_err(|_error| RouteError::BadParameter("to".to_owned()))?;
    EventsQuery::normalize(
        range,
        saw_source.then_some(sources),
        representation.unwrap_or(EventsRepresentation::Groups),
        limit.ok_or_else(|| RouteError::BadParameter("limit".to_owned()))?,
    )
    .map_err(|error| match error {
        kronika_query::EventsQueryError::Limit(_) => RouteError::BadParameter("limit".to_owned()),
        kronika_query::EventsQueryError::Source { .. } => {
            RouteError::BadParameter("source".to_owned())
        }
    })
}

fn parse_row_detail(query: &str) -> Result<String, RouteError> {
    let mut detail_ref = None;
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "detail_ref"
                if detail_ref.is_none()
                    && !value.is_empty()
                    && value.len() <= DETAIL_REF_MAX_ENCODED_BYTES =>
            {
                detail_ref = Some(value);
            }
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    detail_ref.ok_or_else(|| RouteError::BadParameter("detail_ref".to_owned()))
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
