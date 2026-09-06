//! Strict portable parsing of recorded-data resource paths.

use kronika_query::{
    ActiveCursor, CatalogRequest, DETAIL_REF_MAX_ENCODED_BYTES, DataRequest, EventsQuery,
    EventsRepresentation, Filter, HeatmapBatchQuery, HeatmapItemQuery, HeatmapView, HourPart,
    HourRequest, HourSeriesRequest, IndexRequest, MAX_EVENTS_LIMIT, MAX_EVENTS_WINDOW_MICROS,
    NormalizedRanking, Order, QueryError, QueryRequest, RelationGroup, RowsRequest, SegmentRequest,
    SnapshotRequest, StatementScope, TimeRange, Window,
};

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 1_000;
const DEFAULT_SNAPSHOT_PAGE_SIZE: usize = 200;
/// Maximum accepted encoded query-string length.
pub const MAX_QUERY_BYTES: usize = 64 * 1024;
const MAX_SECTION_BYTES: usize = 128;
const MAX_SNAPSHOT_SECTIONS: usize = 16;
/// Maximum number of rows accepted by a snapshot page request.
pub const MAX_SNAPSHOT_PAGE_SIZE: usize = 5_000;
const MAX_SEARCH_EXPRESSION_CHARS: usize = 1_024;
const MAX_FIELDS: usize = 256;
const DEFAULT_HEATMAP_COLUMNS: usize = 60;
const MAX_HEATMAP_COLUMNS: usize = 1_440;
const DEFAULT_HEATMAP_TOP: usize = 25;
const MAX_HEATMAP_TOP: usize = 500;
const MAX_HEATMAP_FIELDS: usize = 4;
const MAX_HEATMAP_GROUP: usize = 4;
const MAX_FILTERS: usize = 64;
const MAX_ORDER_FIELDS: usize = 16;

/// Recorded-data resource requests accepted by native and embedded adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Actual finished/current segment catalog.
    Catalog(CatalogRequest),
    /// One composed timeline hour.
    Hour(HourRequest),
    /// One logical indexed series in one explicit segment.
    Index(IndexRequest),
    /// Projected full-resolution history in one explicit segment.
    History(DataRequest),
    /// One stable page of physical rows in one explicit segment.
    Rows(RowsRequest),
    /// One current-state snapshot in one explicit segment.
    Snapshot(Box<SnapshotRequest>),
    /// The ranked top view of one section over one window.
    Heatmap(HeatmapRequest),
    /// Recorded event groups or physical occurrences over one window.
    Events(EventsQuery),
    /// One exact stored row addressed by an opaque reference.
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
    /// Convert the parsed resource into the existing transport-neutral query.
    ///
    /// # Errors
    ///
    /// Returns the shared semantic refusal for a heatmap range or row-detail
    /// reference that is intentionally validated after route recognition.
    pub fn into_query(self) -> Result<QueryRequest, QueryError> {
        match self {
            Self::Catalog(request) => Ok(QueryRequest::Catalog(request)),
            Self::Hour(request) => Ok(QueryRequest::Hour(request)),
            Self::Index(request) => Ok(QueryRequest::Index(request)),
            Self::History(request) => Ok(QueryRequest::History(request)),
            Self::Rows(request) => Ok(QueryRequest::Rows(request)),
            Self::Snapshot(request) => Ok(QueryRequest::Snapshot(*request)),
            Self::Heatmap(request) => request.into_query().map(QueryRequest::Heatmap),
            Self::Events(request) => Ok(QueryRequest::Events(request)),
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
        return parse_catalog(query).map(Route::Catalog);
    }
    if path == "/api/hour" {
        return parse_hour(query).map(Route::Hour);
    }
    if path == "/api/heatmap" {
        return parse_heatmap(query).map(Route::Heatmap);
    }
    if path == "/api/events" {
        return parse_events(query).map(Route::Events);
    }
    if path == "/api/row-detail" {
        return parse_row_detail(query).map(Route::RowDetail);
    }
    let tail = path
        .strip_prefix("/api/segments/")
        .ok_or(RouteError::NoSuchPath)?;
    let pieces: Vec<&str> = tail.split('/').collect();
    if pieces.len() == 2 && pieces[1] == "snapshot" && !pieces[0].is_empty() {
        return parse_snapshot(number("segment_id", pieces[0])?, query)
            .map(Box::new)
            .map(Route::Snapshot);
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
        "index" if query.is_empty() => Ok(Route::Index(IndexRequest {
            segment_id: segment.segment_id,
            section: segment.section,
        })),
        "index" => Err(RouteError::BadParameter("query".to_owned())),
        "history" => parse_data(segment, query).map(Route::History),
        "rows" => parse_rows(segment, query).map(Route::Rows),
        _ => Err(RouteError::NoSuchPath),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded snapshot query grammar is deliberately parsed in one strict pass"
)]
fn parse_snapshot(segment_id: i64, query: &str) -> Result<SnapshotRequest, RouteError> {
    let mut at = None;
    let mut sections = Vec::new();
    let mut fields = Vec::new();
    let mut by = Vec::new();
    let mut direction = None;
    let mut group = None;
    let mut page_size = None;
    let mut cursor = None;
    let mut search = None;
    let mut first_match = None;
    let mut scope = None;
    let mut text = None;
    let mut filters: Vec<Filter> = Vec::new();
    let mut type_id = None;
    let mut row_ordinal = None;
    for (raw_name, raw_value) in pairs(query)? {
        match raw_name {
            "at" if at.is_none() => at = Some(number("at", raw_value)?),
            "section" => {
                let section = decoded("section", raw_value, false)?;
                if section.is_empty() || section.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                if sections.contains(&section) || sections.len() >= MAX_SNAPSHOT_SECTIONS {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                sections.push(section);
            }
            "field" => {
                let field = decoded("field", raw_value, true)?;
                if field.is_empty() || fields.contains(&field) || fields.len() >= MAX_FIELDS {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(field);
            }
            "by" => {
                let field = decoded("by", raw_value, true)?;
                if field.is_empty() || by.contains(&field) || by.len() >= MAX_ORDER_FIELDS {
                    return Err(RouteError::BadParameter("by".to_owned()));
                }
                by.push(field);
            }
            "direction" if direction.is_none() => {
                direction = Some(match raw_value {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter("direction".to_owned())),
                });
            }
            "group" if group.is_none() => {
                group = Some(match raw_value {
                    "database" => RelationGroup::Database,
                    "schema" => RelationGroup::Schema,
                    "tablespace" => RelationGroup::Tablespace,
                    "object" => RelationGroup::Object,
                    _ => return Err(RouteError::BadParameter("group".to_owned())),
                });
            }
            "type_id" if type_id.is_none() => {
                type_id = Some(unsigned_32("type_id", raw_value)?);
            }
            "row_ordinal" if row_ordinal.is_none() => {
                row_ordinal = Some(unsigned_64("row_ordinal", raw_value)?);
            }
            "text" if text.is_none() => {
                let kept = number("text", raw_value)?;
                if kept <= 0 {
                    return Err(RouteError::BadParameter("text".to_owned()));
                }
                text = Some(kept.unsigned_abs());
            }
            "page_size" if page_size.is_none() => {
                page_size = Some(bounded("page_size", raw_value, MAX_SNAPSHOT_PAGE_SIZE)?);
            }
            "cursor" if cursor.is_none() => {
                let value = decoded("cursor", raw_value, true)?;
                if value.is_empty() {
                    return Err(RouteError::BadParameter("cursor".to_owned()));
                }
                cursor = Some(value);
            }
            "search" if search.is_none() => search = Some(snapshot_search(raw_value)?),
            "scope" if scope.is_none() => scope = Some(statement_scope(raw_value)?),
            "first_match" if first_match.is_none() => {
                if raw_value != "1" {
                    return Err(RouteError::BadParameter("first_match".to_owned()));
                }
                first_match = Some(true);
            }
            other => {
                let name = decoded("parameter", other, true)?;
                let Some(column) = name.strip_prefix("where.") else {
                    return Err(RouteError::BadParameter(name));
                };
                if column.is_empty()
                    || filters.len() >= MAX_FILTERS
                    || filters.iter().any(|filter| filter.column == column)
                {
                    return Err(RouteError::BadParameter("where".to_owned()));
                }
                filters.push(Filter {
                    column: column.to_owned(),
                    value: decoded(&name, raw_value, true)?,
                });
            }
        }
    }
    let first_match = first_match.unwrap_or(false);
    if first_match
        && (sections.as_slice() != ["pg_stat_statements"]
            || fields.as_slice() != ["query"]
            || page_size != Some(1)
            || cursor.is_some()
            || search.is_none()
            || text.is_some()
            || !filters.is_empty()
            || type_id.is_some()
            || row_ordinal.is_some()
            || !by.is_empty()
            || direction.is_some()
            || group.is_some())
    {
        return Err(RouteError::BadParameter("first_match".to_owned()));
    }
    let paged = page_size.is_some()
        || cursor.is_some()
        || search.is_some()
        || !by.is_empty()
        || direction.is_some()
        || group.is_some();
    let scope = scope.unwrap_or_default();
    if scope == StatementScope::Workload
        && (sections.len() != 1
            || !scope.allows_rows(&sections[0])
            || !paged
            || first_match
            || row_ordinal.is_some()
            || group.is_some())
    {
        return Err(RouteError::BadParameter("scope".to_owned()));
    }
    validate_snapshot_shape(&sections, paged, &filters, type_id, row_ordinal, group)?;
    Ok(SnapshotRequest {
        segment_id,
        at: at.ok_or_else(|| RouteError::BadParameter("at".to_owned()))?,
        sections,
        fields,
        by,
        direction: direction.unwrap_or(Order::Desc),
        group,
        page_size: paged.then_some(page_size.unwrap_or(DEFAULT_SNAPSHOT_PAGE_SIZE)),
        cursor,
        search,
        first_match,
        text,
        filters,
        type_id,
        row_ordinal,
        scope,
    })
}

fn snapshot_search(raw: &str) -> Result<String, RouteError> {
    let value = decoded("search", raw, true)?;
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_SEARCH_EXPRESSION_CHARS {
        return Err(RouteError::BadParameter("search".to_owned()));
    }
    Ok(value.to_owned())
}

fn validate_snapshot_shape(
    sections: &[String],
    paged: bool,
    filters: &[Filter],
    type_id: Option<u32>,
    row_ordinal: Option<u64>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    if sections.is_empty() {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if sections.len() != 1
        && (paged || !filters.is_empty() || type_id.is_some() || row_ordinal.is_some())
    {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if row_ordinal.is_some() && (type_id.is_none() || paged || !filters.is_empty()) {
        return Err(RouteError::BadParameter("row_ordinal".to_owned()));
    }
    if group.is_some() && type_id.is_some() {
        return Err(RouteError::BadParameter("type_id".to_owned()));
    }
    if group.is_some()
        && (!paged
            || row_ordinal.is_some()
            || !matches!(sections, [section] if section == "pg_stat_user_tables" || section == "pg_stat_user_indexes"))
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    Ok(())
}

fn parse_heatmap(query: &str) -> Result<HeatmapRequest, RouteError> {
    let mut from = None;
    let mut to = None;
    let mut section = None;
    let mut fields: Vec<String> = Vec::new();
    let mut columns = None;
    let mut top = None;
    let mut group: Vec<String> = Vec::new();
    let mut type_id = None;
    let mut scope = None;
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if from.is_none() => from = Some(number("from", &value)?),
            "to" if to.is_none() => to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "scope" if scope.is_none() => scope = Some(statement_scope(&value)?),
            "field" => {
                if value.is_empty() || fields.contains(&value) || fields.len() >= MAX_HEATMAP_FIELDS
                {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(value);
            }
            "columns" if columns.is_none() => {
                columns = Some(bounded("columns", &value, MAX_HEATMAP_COLUMNS)?);
            }
            "top" if top.is_none() => top = Some(bounded("top", &value, MAX_HEATMAP_TOP)?),
            "group" => {
                if value.is_empty() || group.contains(&value) || group.len() >= MAX_HEATMAP_GROUP {
                    return Err(RouteError::BadParameter("group".to_owned()));
                }
                group.push(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    if from > to {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    if fields.is_empty() {
        return Err(RouteError::BadParameter("field".to_owned()));
    }
    Ok(HeatmapRequest {
        from,
        to,
        section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
        fields,
        columns: columns.unwrap_or(DEFAULT_HEATMAP_COLUMNS),
        top: top.unwrap_or(DEFAULT_HEATMAP_TOP),
        group,
        type_id,
        scope: scope.unwrap_or_default(),
    })
}

/// Only the `postgresql_summary` series folds statements, so only it accepts a scope.
fn summary_scope(
    scope: Option<StatementScope>,
    section: Option<&str>,
) -> Result<StatementScope, RouteError> {
    let scope = scope.unwrap_or_default();
    if !scope.allows_series(section) {
        return Err(RouteError::BadParameter("scope".to_owned()));
    }
    Ok(scope)
}

fn statement_scope(value: &str) -> Result<StatementScope, RouteError> {
    StatementScope::parse(value).ok_or_else(|| RouteError::BadParameter("scope".to_owned()))
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

fn bounded(name: &str, value: &str, cap: usize) -> Result<usize, RouteError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|parsed| (1..=cap).contains(parsed))
        .ok_or_else(|| RouteError::BadParameter(name.to_owned()))
}

fn parse_catalog(query: &str) -> Result<CatalogRequest, RouteError> {
    let mut window = Window::default();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    valid_window(window).map(|window| CatalogRequest { window })
}

fn parse_hour(query: &str) -> Result<HourRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut window = Window::default();
    let mut section = None;
    let mut type_id = None;
    let mut group = None;
    let mut part = HourPart::Combined;
    let mut segments = None;
    let mut active = None;
    let mut saw_part = false;
    let mut scope = None;
    for (name, value) in extras {
        match name.as_str() {
            "scope" if scope.is_none() => scope = Some(statement_scope(&value)?),
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            "group" if group.is_none() => {
                group = Some(match value.as_str() {
                    "database" => RelationGroup::Database,
                    "schema" => RelationGroup::Schema,
                    "tablespace" => RelationGroup::Tablespace,
                    "object" => RelationGroup::Object,
                    _ => return Err(RouteError::BadParameter("group".to_owned())),
                });
            }
            "part" if !saw_part => {
                part = match value.as_str() {
                    "base" => HourPart::Base,
                    "lanes" => HourPart::Lanes,
                    _ => return Err(RouteError::BadParameter("part".to_owned())),
                };
                saw_part = true;
            }
            "segments" if segments.is_none() => segments = Some(segment_ids(&value)?),
            "active" if active.is_none() => active = Some(active_cursor("active", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let window = valid_window(window)?;
    let scope = summary_scope(scope, section.as_deref())?;
    let Some(section) = section else {
        if fields.is_empty() && filters.is_empty() && type_id.is_none() && group.is_none() {
            if part == HourPart::Lanes && segments.is_none() {
                return Err(RouteError::BadParameter("segments".to_owned()));
            }
            if part == HourPart::Lanes && (window.from.is_none() || window.to.is_none()) {
                return Err(RouteError::BadParameter(
                    if window.from.is_none() { "from" } else { "to" }.to_owned(),
                ));
            }
            if part != HourPart::Lanes && (segments.is_some() || active.is_some()) {
                return Err(RouteError::BadParameter(
                    if segments.is_some() {
                        "segments"
                    } else {
                        "active"
                    }
                    .to_owned(),
                ));
            }
            return Ok(HourRequest {
                window,
                series: None,
                part,
                segments,
                active,
            });
        }
        return Err(RouteError::BadParameter("section".to_owned()));
    };
    if part != HourPart::Combined || segments.is_some() || active.is_some() {
        return Err(RouteError::BadParameter(if segments.is_some() {
            "segments".to_owned()
        } else if active.is_some() {
            "active".to_owned()
        } else {
            "part".to_owned()
        }));
    }
    validate_relation_series(&section, &fields, &filters, type_id, group)?;
    Ok(HourRequest {
        window,
        series: Some(HourSeriesRequest {
            section,
            fields,
            filters,
            type_id,
            group,
            scope,
        }),
        part,
        segments,
        active,
    })
}

fn valid_window(window: Window) -> Result<Window, RouteError> {
    if window
        .from
        .zip(window.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    Ok(window)
}

fn segment_ids(value: &str) -> Result<Vec<i64>, RouteError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for value in value.split(',') {
        let id = number("segments", value)?;
        if ids.contains(&id) {
            return Err(RouteError::BadParameter("segments".to_owned()));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn validate_relation_series(
    section: &str,
    fields: &[String],
    filters: &[Filter],
    type_id: Option<u32>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    let Some(group) = group else {
        return Ok(());
    };
    if type_id.is_some()
        || fields.is_empty()
        || !matches!(section, "pg_stat_user_tables" | "pg_stat_user_indexes")
        || group == RelationGroup::Object
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    let required: &[&str] = match group {
        RelationGroup::Database => &["datid"],
        RelationGroup::Schema => &["datid", "schemaname"],
        RelationGroup::Tablespace => &["tablespace_oid"],
        RelationGroup::Object => unreachable!(),
    };
    if filters.len() != required.len()
        || required
            .iter()
            .any(|name| !filters.iter().any(|filter| filter.column == *name))
    {
        return Err(RouteError::BadParameter("where".to_owned()));
    }
    if group == RelationGroup::Tablespace
        && filters[0]
            .value
            .parse::<u32>()
            .ok()
            .is_none_or(|oid| oid == 0)
    {
        return Err(RouteError::BadParameter("where.tablespace_oid".to_owned()));
    }
    Ok(())
}

fn parse_data(segment: SegmentRequest, query: &str) -> Result<DataRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut after = None;
    let mut type_id = None;
    for (name, value) in extras {
        match name.as_str() {
            "after" if after.is_none() => after = Some(active_cursor("after", &value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(DataRequest {
        segment,
        fields,
        filters,
        type_id,
        after,
    })
}

fn parse_rows(segment: SegmentRequest, query: &str) -> Result<RowsRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut order = Order::Asc;
    let mut page_size = DEFAULT_PAGE_SIZE;
    let mut cursor = None;
    let mut after = None;
    let mut type_id = None;
    let mut saw_order = false;
    let mut saw_page_size = false;
    for (name, value) in extras {
        match name.as_str() {
            "order" if !saw_order => {
                order = match value.as_str() {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter(name)),
                };
                saw_order = true;
            }
            "page_size" if !saw_page_size => {
                page_size = bounded("page_size", &value, MAX_PAGE_SIZE)?;
                saw_page_size = true;
            }
            "cursor" if cursor.is_none() && !value.is_empty() => cursor = Some(value),
            "after" if after.is_none() => after = Some(active_cursor("after", &value)?),
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(RowsRequest {
        data: DataRequest {
            segment,
            fields,
            filters,
            type_id,
            after,
        },
        order,
        page_size,
        cursor,
    })
}

fn active_cursor(name: &str, value: &str) -> Result<ActiveCursor, RouteError> {
    let (segment_id, wal_position) = value
        .split_once(',')
        .filter(|(segment_id, wal_position)| {
            !segment_id.is_empty() && !wal_position.is_empty() && !wal_position.contains(',')
        })
        .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
    Ok(ActiveCursor {
        segment_id: number(name, segment_id)?,
        wal_position: wal_position
            .parse()
            .map_err(|_error| RouteError::BadParameter(name.to_owned()))?,
    })
}

fn validate_after(segment: &SegmentRequest, after: Option<ActiveCursor>) -> Result<(), RouteError> {
    if after.is_some_and(|cursor| cursor.segment_id != segment.segment_id) {
        return Err(RouteError::BadParameter("after".to_owned()));
    }
    Ok(())
}

type Extra = (String, String);
type DataParameters = (Vec<String>, Vec<Filter>, Vec<Extra>);

fn data_parameters(query: &str) -> Result<DataParameters, RouteError> {
    let mut fields = Vec::new();
    let mut filters = Vec::new();
    let mut extras = Vec::new();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        if name == "field" {
            if value.is_empty() || fields.len() >= MAX_FIELDS || fields.contains(&value) {
                return Err(RouteError::BadParameter("field".to_owned()));
            }
            fields.push(value);
        } else if let Some(column) = name.strip_prefix("where.") {
            if column.is_empty()
                || filters.len() >= MAX_FILTERS
                || filters
                    .iter()
                    .any(|filter: &Filter| filter.column == column)
            {
                return Err(RouteError::BadParameter("where".to_owned()));
            }
            filters.push(Filter {
                column: column.to_owned(),
                value,
            });
        } else {
            extras.push((name, value));
        }
    }
    Ok((fields, filters, extras))
}

fn pairs(query: &str) -> Result<Vec<(&str, &str)>, RouteError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|part| {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            Ok(part.split_once('=').unwrap_or((part, "")))
        })
        .collect()
}

fn decoded(name: &str, value: &str, plus_as_space: bool) -> Result<String, RouteError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0_usize;
    while at < bytes.len() {
        match bytes[at] {
            b'+' if plus_as_space => {
                out.push(b' ');
                at += 1;
            }
            b'%' => {
                let raw = bytes
                    .get(at + 1..at + 3)
                    .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
                let text = std::str::from_utf8(raw)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                let byte = u8::from_str_radix(text, 16)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                out.push(byte);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn unsigned_32(name: &str, value: &str) -> Result<u32, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn unsigned_64(name: &str, value: &str) -> Result<u64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

#[cfg(test)]
mod tests;
