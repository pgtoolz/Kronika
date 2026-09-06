//! Reads one snapshot and derives counter rates.

mod relation;
mod search;
mod selector;

pub use relation::RelationRow;
pub use search::{
    Expr, GlobPattern, Quantity, QuantityKind, ResultField, SEARCH_MAX_CLAUSES,
    SEARCH_MAX_VALUE_CHARS, SearchClause, SearchDiagnostic, SearchField, SearchFieldKind,
    SearchOperator, SearchValue, StructuredSearch, result_field, search_fields,
    search_value_matches, valid_identifier,
};
pub use selector::{
    CurrentSnapshotQuery, FinderOrder, FinderQuery, FinderResult, FinderSurface, SnapshotPoint,
    execute_current_plain, execute_current_relation, execute_plain, execute_processes,
    execute_relation,
};

use std::borrow::Cow;
use std::cmp::Ordering;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell as Counter;

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment, SegmentKind};
use kronika_registry::{ColumnClass, ColumnType, contract, logical_section_name};
use serde_json::{Value, json};

use crate::StatementScope;
use crate::dataset::{DatasetSegment, QueryDataset, SegmentBounds, SegmentSelection};
use crate::exact_product::compare_products;
use crate::output_fields as shared_fields;
use crate::projection::{Plan, plans, resolved_dictionary};
use crate::render::{cell, projected_layout, record, shorten};
use crate::statement_scope::{CollectorStatements, plan_statement_query_id_columns};
use crate::{
    DataRequest, Order, Prepared, QueryContext, QueryError, QueryExecution, QueryIdentity,
    QueryMetadata, QuerySink, QueryStability, RelationGroup, SegmentRequest, SnapshotRequest,
};

pub(crate) struct PreparedSnapshot {
    dataset: Arc<dyn QueryDataset>,
    anchor: DatasetSegment,
    pin_current: bool,
    prior_sources: Vec<DatasetSegment>,
    relation_predecessors: Vec<DatasetSegment>,
    relation_moments: RetainedRelationMoments,
    at: i64,
    current_from: Option<i64>,
    sections: Vec<SectionPlans>,
    relation_filters: Vec<crate::Filter>,
    by: Vec<String>,
    direction: Order,
    group: Option<RelationGroup>,
    relation_fields: Vec<String>,
    page_size: Option<usize>,
    cursor: Option<SnapshotCursor>,
    binding: u64,
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
    text: Option<u64>,
    row_ordinal: Option<u64>,
    scope: StatementScope,
    stability: QueryStability,
    validator_shape: String,
    validator_segments: Vec<DatasetSegment>,
}

type Readings = BTreeMap<Vec<IdentityCell>, CounterReadings>;
type CounterReadings = BTreeMap<&'static str, Cell>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum IdentityCell {
    Null,
    I16(i16),
    I32(i32),
    I64(i64),
    Ts(i64),
    U32(u32),
    U64(u64),
    F64(u64),
    Bool(bool),
    StrId(u64),
    ListI32(Vec<i32>),
}

struct StagedRow {
    ordinal: u64,
    row: Row,
    identity: Vec<IdentityCell>,
}

#[derive(Clone, Copy)]
struct RowCoordinate {
    segment_id: i64,
    ordinal: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapshotCursor {
    segment_id: i64,
    active_position: u64,
    context_index: usize,
    ordinal: u64,
    binding: u64,
}

struct PageRows {
    limit: usize,
    rows: BinaryHeap<Reverse<PageRankedRow>>,
}

struct PageRankedRow {
    staged: PageStagedRow,
    value: Option<PageOrderValue>,
    direction: Order,
}

struct PageStagedRow {
    context_index: usize,
    ordinal: u64,
    row: Row,
    identity: Vec<IdentityCell>,
}

enum PageOrderValue {
    Integer(i128),
    Float(f64),
    IntegerRate { delta: i128, elapsed: i64 },
    FloatRate(f64),
    IntegerRatio { numerator: u128, denominator: u128 },
    FloatRatio(f64),
    Text(Vec<u8>),
}

#[derive(Clone)]
struct PageOrder {
    name: &'static str,
    kind: PageOrderKind,
}

#[derive(Clone)]
enum PageOrderKind {
    Column(&'static str),
    CounterRatio {
        numerator: Vec<&'static str>,
        denominator: Vec<&'static str>,
        neutral_nulls: bool,
    },
    ValueRatio {
        numerator: Vec<&'static str>,
        denominator: Vec<&'static str>,
    },
}

enum RowWindow {
    Untimed,
    Shared {
        timestamp: &'static str,
        current: i64,
    },
    Partitioned {
        timestamp: &'static str,
        column: &'static str,
        current: BTreeMap<IdentityCell, i64>,
    },
}

#[derive(Clone, Copy)]
struct RateContext<'a> {
    previous: Option<&'a Readings>,
    elapsed: Option<i64>,
}

struct SectionPlans {
    logical_name: String,
    plans: Vec<Plan>,
}

struct PageContext<'a> {
    context_index: usize,
    plan: &'a Plan,
    logical_name: &'a str,
    source: &'a DatasetSegment,
    rows: u64,
    window: RowWindow,
    previous: Option<Arc<Readings>>,
    elapsed: Option<i64>,
    elapsed_by_partition: Arc<BTreeMap<IdentityCell, i64>>,
    sample_from: Option<i64>,
    sample_to: Option<i64>,
    order: Option<PageOrder>,
    search_columns: Vec<&'static str>,
    clock_ticks_per_second: Option<u128>,
    block_size: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PartitionSource {
    Earlier(usize),
    Current,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocatedMoment {
    at: i64,
    sources: BTreeSet<PartitionSource>,
}

#[derive(Clone)]
struct PartitionMoments {
    current: LocatedMoment,
    previous: Option<LocatedMoment>,
}

struct SelectedPartition {
    layout_index: usize,
    type_id: u32,
    moments: PartitionMoments,
}

struct PartitionRateState {
    previous: Arc<Readings>,
    elapsed_by_partition: Arc<BTreeMap<IdentityCell, i64>>,
    sample_from: Option<i64>,
    sample_to: Option<i64>,
}

#[derive(Clone, Copy)]
struct SnapshotViewSpec {
    temporal_partition: &'static str,
}

impl SnapshotViewSpec {
    fn for_logical_name(logical_name: &str) -> Option<Self> {
        match logical_name {
            "pg_stat_user_tables" | "pg_stat_user_indexes" => Some(Self {
                temporal_partition: "datid",
            }),
            _ => None,
        }
    }
}

impl RowWindow {
    fn matches(&self, row: &Row) -> bool {
        match self {
            Self::Untimed => true,
            Self::Shared { timestamp, current } => row_timestamp(row, timestamp) == Some(*current),
            Self::Partitioned {
                timestamp,
                column,
                current,
            } => row.get(column).is_some_and(|partition| {
                current
                    .get(&identity_cell(partition))
                    .is_some_and(|at| row_timestamp(row, timestamp) == Some(*at))
            }),
        }
    }
}

fn rows_of(reference: &DatasetSegment, type_id: u32) -> Option<u64> {
    reference
        .sections()
        .iter()
        .find(|section| section.type_id == type_id)
        .map(|section| section.rows)
}

impl PageContext<'_> {
    fn elapsed_for(&self, row: &Row) -> Option<i64> {
        match &self.window {
            RowWindow::Partitioned { column, .. } => row
                .get(column)
                .and_then(|partition| self.elapsed_by_partition.get(&identity_cell(partition)))
                .copied(),
            RowWindow::Untimed | RowWindow::Shared { .. } => self.elapsed,
        }
    }

    fn predecessor<'a>(
        &'a self,
        row: &Row,
        identity: &[IdentityCell],
    ) -> Option<&'a CounterReadings> {
        let before = self.previous.as_ref()?.get(identity)?;
        if let Some(column) = self.plan.contract.column("starttime")
            && row.get(column.name)? != before.get(column.name)?
        {
            return None;
        }
        Some(before)
    }
}

struct PageMetadata {
    eligible: u64,
    excluded: u64,
    returned: usize,
    has_more: bool,
    next_cursor: Option<String>,
    page_size: usize,
}

#[derive(Clone, Copy, Default)]
struct PageFacts {
    clock_ticks_per_second: CachedFact,
    block_size: CachedFact,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CachedFact {
    #[default]
    Unknown,
    Absent,
    Value(u128),
}

impl CachedFact {
    const fn value(self) -> Option<u128> {
        match self {
            Self::Value(value) => Some(value),
            Self::Unknown | Self::Absent => None,
        }
    }
}

// One bounded batch covers the maximum public page while amortizing projected
// section and dictionary reads for snapshot and relation-history consumers.
const SNAPSHOT_CHUNK_ROWS: usize = 1_024;
const PROCESS_USER_TYPE_ID: u32 = 1_124_002;
const PROCESS_VIRTUAL_FIELDS: &[&str] = &["user", "effective_user", "cpu_time_ticks"];
const PROCESS_USER_VIRTUAL_FIELDS: &[&str] = &["user", "effective_user"];
const CPU_TIME_VIRTUAL_FIELD: &str = "cpu_time_ticks";
const MAX_PROCESS_USERS: usize = 4 * 1024;

#[derive(Default)]
struct ProcessUsers {
    names: HashMap<(u8, u32), String>,
}

impl ProcessUsers {
    fn load(segment: &Segment, plan: &Plan) -> Result<Self, QueryError> {
        if plan.contract.name != "os_process" || segment.rows_of(PROCESS_USER_TYPE_ID).is_none() {
            return Ok(Self::default());
        }
        let mut encoded = Vec::new();
        let mut ids = HashSet::new();
        segment.visit_rows(
            PROCESS_USER_TYPE_ID,
            &["uid", "username", "scope"],
            0,
            MAX_PROCESS_USERS.saturating_add(1),
            |_ordinal, row| {
                let (Some(Cell::U32(uid)), Some(Cell::StrId(username)), Some(Cell::U32(scope))) =
                    (row.get("uid"), row.get("username"), row.get("scope"))
                else {
                    return true;
                };
                ids.insert(*username);
                encoded.push((*scope, *uid, *username));
                encoded.len() <= MAX_PROCESS_USERS
            },
        )?;
        if encoded.len() > MAX_PROCESS_USERS {
            return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "os_user exceeds the per-segment mapping limit",
            ))));
        }
        let dictionary = resolved_dictionary(segment, &ids)?;
        let mut names = HashMap::with_capacity(encoded.len());
        for (scope, uid, username) in encoded {
            let Ok(scope) = u8::try_from(scope) else {
                continue;
            };
            let Some(Resolved::Str(bytes)) = dictionary.resolve(username) else {
                continue;
            };
            let Ok(username) = std::str::from_utf8(bytes) else {
                continue;
            };
            names
                .entry((scope, uid))
                .or_insert_with(|| username.to_owned());
        }
        Ok(Self { names })
    }

    fn for_row<'a>(&'a self, row: &Row, uid_column: &str) -> Option<&'a str> {
        let (Some(Cell::U32(scope)), Some(Cell::U32(uid))) =
            (row.get("scope"), row.get(uid_column))
        else {
            return None;
        };
        let scope = u8::try_from(*scope).ok()?;
        self.names.get(&(scope, *uid)).map(String::as_str)
    }
}

#[cfg(test)]
thread_local! {
    static PAGE_CHUNK_ROWS: Counter<usize> = const { Counter::new(0) };
    static PAGE_SOURCE_VISITS: Counter<usize> = const { Counter::new(0) };
    static PAGE_CANDIDATE_DICTIONARIES: Counter<usize> = const { Counter::new(0) };
    static FIRST_MATCH_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_CHUNK_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_STAGED_ROWS: Counter<usize> = const { Counter::new(0) };
    static CONTEXT_SELECTION_DICTIONARIES: Counter<usize> = const { Counter::new(0) };
    static RELATION_MOMENT_VISITS: Counter<usize> = const { Counter::new(0) };
    static PARTITION_PREDECESSOR_VISITS: Counter<usize> = const { Counter::new(0) };
    static RELATION_PROJECTED_METRICS: Counter<usize> = const { Counter::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_page_operations() {
    PAGE_CHUNK_ROWS.set(0);
    PAGE_SOURCE_VISITS.set(0);
    PAGE_CANDIDATE_DICTIONARIES.set(0);
}

#[cfg(test)]
pub(crate) fn page_operations() -> (usize, usize, usize) {
    (
        PAGE_SOURCE_VISITS.get(),
        PAGE_CANDIDATE_DICTIONARIES.get(),
        PAGE_CHUNK_ROWS.get(),
    )
}

#[cfg(test)]
pub(crate) fn reset_first_match_rows() {
    FIRST_MATCH_ROWS.set(0);
}

#[cfg(test)]
pub(crate) fn first_match_rows() -> usize {
    FIRST_MATCH_ROWS.get()
}

#[cfg(test)]
pub(crate) fn reset_context_operations() {
    CONTEXT_CHUNK_ROWS.set(0);
    CONTEXT_STAGED_ROWS.set(0);
    CONTEXT_SELECTION_DICTIONARIES.set(0);
}

#[cfg(test)]
pub(crate) fn context_operations() -> (usize, usize, usize) {
    (
        CONTEXT_CHUNK_ROWS.get(),
        CONTEXT_STAGED_ROWS.get(),
        CONTEXT_SELECTION_DICTIONARIES.get(),
    )
}

#[cfg(test)]
pub(crate) fn reset_relation_snapshot_operations() {
    RELATION_MOMENT_VISITS.set(0);
    PARTITION_PREDECESSOR_VISITS.set(0);
    RELATION_PROJECTED_METRICS.set(0);
}

#[cfg(test)]
pub(crate) fn relation_snapshot_operations() -> (usize, usize, usize) {
    (
        RELATION_MOMENT_VISITS.get(),
        PARTITION_PREDECESSOR_VISITS.get(),
        RELATION_PROJECTED_METRICS.get(),
    )
}

/// Snapshot selection and validator facts available before predecessor scans.
#[derive(Debug)]
pub struct SnapshotPreparation {
    dataset: Arc<dyn QueryDataset>,
    anchor: DatasetSegment,
    segments: Vec<DatasetSegment>,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
    cursor: Option<SnapshotCursor>,
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
    binding: u64,
    stability: QueryStability,
    validator_shape: String,
    validator_segments: Vec<DatasetSegment>,
}

struct PreparedSnapshotInputs {
    cursor: Option<SnapshotCursor>,
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
    binding: u64,
}

/// Capture one explicit snapshot selection without scanning predecessors.
///
/// # Errors
///
/// Returns a query error when the request is invalid, its segment is absent,
/// or the captured dataset cannot provide a consistent catalog.
pub fn prepare_snapshot(
    context: &QueryContext,
    request: SnapshotRequest,
) -> Result<SnapshotPreparation, QueryError> {
    let inputs = prepared_snapshot_inputs(&request)?;
    let listing = {
        let catalog = context.dataset.catalog()?;
        catalog.segments(SegmentSelection::new(SegmentBounds::all()))?
    };
    let clean = listing.warnings.is_empty();
    let mut segments = listing.segments;
    let index = segments
        .iter()
        .position(|segment| segment.id() == request.segment_id)
        .ok_or(QueryError::NoSuchSegment)?;
    let current = segments.remove(index);
    prepare_selected_state_with_inputs(
        Arc::clone(&context.dataset),
        current,
        segments,
        clean,
        request,
        true,
        None,
        inputs,
    )
}

pub(super) fn prepare_selected(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    current_from: Option<i64>,
) -> Result<PreparedSnapshot, QueryError> {
    prepare_selected_state(
        dataset,
        current,
        segments,
        clean,
        request,
        false,
        current_from,
    )?
    .finish_prepared()
}

fn prepare_selected_state(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
) -> Result<SnapshotPreparation, QueryError> {
    let inputs = prepared_snapshot_inputs(&request)?;
    prepare_selected_state_with_inputs(
        dataset,
        current,
        segments,
        clean,
        request,
        pin_current,
        current_from,
        inputs,
    )
}

fn prepared_snapshot_inputs(
    request: &SnapshotRequest,
) -> Result<PreparedSnapshotInputs, QueryError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(SnapshotCursor::parse)
        .transpose()?;
    let search = prepared_search(request, cursor.is_some())?;
    let first_match_query_id = prepared_first_match(request, search.as_deref())?;
    let binding = snapshot_binding(request, search.as_deref());
    let parsed = cursor
        .filter(|cursor| cursor.segment_id == request.segment_id && cursor.binding == binding);
    if request.cursor.is_some() && parsed.is_none() {
        return Err(QueryError::BadCursor);
    }
    Ok(PreparedSnapshotInputs {
        cursor: parsed,
        search,
        first_match_query_id,
        binding,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "captured selection and parsed request state meet at this ownership boundary"
)]
fn prepare_selected_state_with_inputs(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
    inputs: PreparedSnapshotInputs,
) -> Result<SnapshotPreparation, QueryError> {
    let PreparedSnapshotInputs {
        cursor,
        search,
        first_match_query_id,
        binding,
    } = inputs;
    let anchor = pin(dataset.as_ref(), current, cursor)?;
    let active_position = anchor.active_position().unwrap_or(0);
    if cursor.is_some_and(|cursor| cursor.active_position != active_position) {
        return Err(QueryError::BadCursor);
    }
    let validator_segments =
        std::iter::once(&anchor)
            .chain(segments.iter().filter(|candidate| {
                candidate.id() < anchor.id() && candidate.min_ts() <= request.at
            }))
            .cloned()
            .collect::<Vec<_>>();
    let immutable = clean
        && anchor.kind() == SegmentKind::Finished
        && validator_segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
    let stability = match anchor.kind() {
        SegmentKind::Active => QueryStability::Mutable,
        SegmentKind::Finished if immutable => QueryStability::Immutable,
        SegmentKind::Finished => QueryStability::Revalidate,
    };
    let validator_shape = format!("{request:?}");
    Ok(SnapshotPreparation {
        dataset,
        anchor,
        segments,
        request,
        pin_current,
        current_from,
        cursor,
        search,
        first_match_query_id,
        binding,
        stability,
        validator_shape,
        validator_segments,
    })
}

impl SnapshotPreparation {
    /// Stability and immutable identity for HTTP cache adaptation.
    #[must_use]
    pub fn metadata(&self) -> QueryMetadata<'_> {
        QueryMetadata {
            stability: self.stability,
            identity: (self.stability == QueryStability::Immutable).then_some(
                QueryIdentity::SegmentSet {
                    resource: "snapshot",
                    shape: &self.validator_shape,
                    segments: &self.validator_segments,
                },
            ),
        }
    }

    /// Finish planning and predecessor scans, producing the shared execution.
    ///
    /// # Errors
    ///
    /// Returns a query error when projection planning, segment opening, or
    /// predecessor selection fails.
    pub fn finish(self) -> Result<QueryExecution, QueryError> {
        Ok(QueryExecution {
            prepared: Prepared::Snapshot(self.finish_prepared()?),
        })
    }

    fn finish_prepared(self) -> Result<PreparedSnapshot, QueryError> {
        let Self {
            dataset,
            anchor,
            segments,
            request,
            pin_current,
            current_from,
            cursor,
            search,
            first_match_query_id,
            binding,
            stability,
            validator_shape,
            validator_segments,
        } = self;
        let segment = dataset.open(&anchor)?;
        let relation_fields = request
            .group
            .map(|group| shared_fields(&request.sections, group, &request.fields))
            .transpose()?
            .unwrap_or_default();
        let (physical_filters, relation_filters) = relation::split_filters(&request)?;
        let mut physical_request = request.clone();
        physical_request.filters = physical_filters;
        let mut sections = section_plans(
            &segment,
            &physical_request,
            &relation_fields,
            search.as_deref(),
        )?;
        project_statement_text(&mut sections, request.scope);
        let has_relation = sections
            .iter()
            .any(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some());
        let has_other = sections
            .iter()
            .any(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_none());
        let prior_sources = if has_other {
            preceding(
                dataset.as_ref(),
                &anchor,
                segments.clone(),
                &segment,
                &sections,
                request.at,
                pin_current,
            )?
        } else {
            Vec::new()
        };
        let (relation_predecessors, relation_moments) = if has_relation {
            relation_preceding(
                dataset.as_ref(),
                &anchor,
                segments,
                &segment,
                &sections,
                &physical_request.filters,
                request.at,
            )?
        } else {
            (Vec::new(), BTreeMap::new())
        };
        validate_search_projection(search.as_deref(), &sections)?;
        validate_exact_locator(&segment, &request, &sections)?;
        drop(segment);
        Ok(PreparedSnapshot {
            dataset,
            anchor,
            pin_current,
            prior_sources,
            relation_predecessors,
            relation_moments,
            at: request.at,
            current_from,
            sections,
            relation_filters,
            by: request.by,
            direction: request.direction,
            group: request.group,
            relation_fields,
            page_size: request.page_size,
            cursor,
            binding,
            search,
            first_match_query_id,
            text: request.text,
            row_ordinal: request.row_ordinal,
            scope: request.scope,
            stability,
            validator_shape,
            validator_segments,
        })
    }
}

fn prepared_first_match(
    request: &SnapshotRequest,
    search: Option<&StructuredSearch>,
) -> Result<Option<i64>, QueryError> {
    if !request.first_match {
        return Ok(None);
    }
    search
        .and_then(StructuredSearch::first_match_query_id)
        .map(Some)
        .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))
}

fn prepared_search(
    request: &SnapshotRequest,
    cursor_present: bool,
) -> Result<Option<Box<StructuredSearch>>, QueryError> {
    let parsed = request
        .search
        .as_deref()
        .map(|raw| {
            let [logical_name] = request.sections.as_slice() else {
                return Err(QueryError::BadFilter("search".to_owned()));
            };
            let search = StructuredSearch::parse(raw, logical_name).map_err(|_diagnostic| {
                if cursor_present {
                    QueryError::BadCursor
                } else {
                    QueryError::BadFilter("search".to_owned())
                }
            })?;
            if request
                .group
                .is_some_and(|group| group != RelationGroup::Object)
            {
                search.validate_grouped_phase().map_err(|_diagnostic| {
                    if cursor_present {
                        QueryError::BadCursor
                    } else {
                        QueryError::BadFilter("search".to_owned())
                    }
                })?;
            }
            Ok(search)
        })
        .transpose()?;
    Ok(parsed.map(Box::new))
}

/// A scoped statement page reads the statement text so the collector's own
/// rows can be recognised while the page is ranked.
fn project_statement_text(sections: &mut [SectionPlans], scope: StatementScope) {
    for section in sections {
        if !scope.filters(&section.logical_name) {
            continue;
        }
        for plan in &mut section.plans {
            if let Some(column) = plan.contract.column("query") {
                plan.add_projection_columns(&[column.name]);
            }
        }
    }
}

fn section_plans(
    segment: &Segment,
    request: &SnapshotRequest,
    relation_fields: &[String],
    search: Option<&StructuredSearch>,
) -> Result<Vec<SectionPlans>, QueryError> {
    let shared_projection = request.sections.len() > 1 && !request.fields.is_empty();
    if shared_projection {
        validate_shared_projection(segment, &request.sections, &request.fields)?;
    }
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in &request.sections {
        let fields = if let Some(group) = request.group {
            let wanted = relation::snapshot_physical_fields(
                logical_name,
                group,
                relation_fields,
                &request.by,
                search,
            )?;
            section_projection(segment, logical_name, &wanted)
        } else if shared_projection {
            section_projection(segment, logical_name, &request.fields)
        } else {
            request.fields.clone()
        };
        if shared_projection && fields.is_empty() {
            continue;
        }
        let selected_virtual = if logical_name == "os_process" {
            if fields.is_empty() {
                PROCESS_VIRTUAL_FIELDS.to_vec()
            } else {
                fields
                    .iter()
                    .filter_map(|field| {
                        PROCESS_VIRTUAL_FIELDS
                            .contains(&field.as_str())
                            .then_some(field.as_str())
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };
        let physical_fields = fields
            .iter()
            .filter(|field| !PROCESS_VIRTUAL_FIELDS.contains(&field.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields: physical_fields.clone(),
            filters: request.filters.clone(),
            type_id: request.type_id,
            after: None,
        };
        // Missing sections are empty so one source cannot fail the snapshot.
        match plans(segment, &data, true) {
            Ok(mut plans) => {
                for plan in &mut plans {
                    if !fields.is_empty() {
                        plan.retain_output_fields(&physical_fields);
                    }
                    for field in &selected_virtual {
                        plan.add_virtual_output(field);
                    }
                    if !fields.is_empty() {
                        plan.order_output_fields(&fields);
                    }
                    if selected_virtual
                        .iter()
                        .any(|field| PROCESS_USER_VIRTUAL_FIELDS.contains(field))
                    {
                        plan.add_projection_columns(&["uid", "euid", "scope"]);
                    }
                    if selected_virtual.contains(&CPU_TIME_VIRTUAL_FIELD) {
                        plan.add_projection_columns(&["utime", "stime"]);
                    }
                    if logical_name == "pg_store_plans" {
                        plan.add_aliased_output("calls_per_second", "calls");
                    }
                    let order = page_order(logical_name, plan, &request.by);
                    plan.add_projection_columns(
                        &order.as_ref().map_or_else(Vec::new, PageOrder::columns),
                    );
                    if logical_name == "os_process" && plan.contract.column("starttime").is_some() {
                        plan.add_projection_columns(&["starttime"]);
                    }
                    if let Some(search) = search {
                        plan.add_projection_columns(&search_columns(logical_name, plan, search));
                    }
                }
                sections.push(SectionPlans {
                    logical_name: logical_name.clone(),
                    plans,
                });
            }
            Err(QueryError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(sections)
}

impl PageOrder {
    fn columns(&self) -> Vec<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column) => vec![*column],
            PageOrderKind::CounterRatio {
                numerator,
                denominator,
                ..
            }
            | PageOrderKind::ValueRatio {
                numerator,
                denominator,
                ..
            } => numerator.iter().chain(denominator).copied().collect(),
        }
    }

    fn dictionary_column(&self, plan: &Plan) -> Option<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column)
                if plan
                    .contract
                    .column(column)
                    .is_some_and(|column| column.ty == ColumnType::StrId) =>
            {
                Some(*column)
            }
            PageOrderKind::Column(_)
            | PageOrderKind::CounterRatio { .. }
            | PageOrderKind::ValueRatio { .. } => None,
        }
    }
}

fn page_order(logical_name: &str, plan: &Plan, requested: &[String]) -> Option<PageOrder> {
    requested.iter().find_map(|name| {
        plan.contract
            .column(name)
            .map(|column| PageOrder {
                name: column.name,
                kind: PageOrderKind::Column(column.name),
            })
            .or_else(|| derived_page_order(logical_name, plan, name))
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "all fixed derived sort tokens remain visibly allowlisted together"
)]
fn derived_page_order(logical_name: &str, plan: &Plan, token: &str) -> Option<PageOrder> {
    let supported = match logical_name {
        "pg_stat_statements" => matches!(plan.type_id, 1_002_001..=1_002_006),
        "pg_store_plans" => matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001),
        "pg_stat_user_tables" => matches!(plan.type_id, 1_013_005..=1_013_008),
        "pg_stat_user_indexes" => matches!(plan.type_id, 1_014_003..=1_014_004),
        _ => false,
    };
    if !supported {
        return None;
    }
    let column = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| plan.contract.column(name).map(|column| column.name))
    };
    let columns = |names: &[&str]| {
        names
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect::<Vec<_>>()
    };
    let counters =
        |name, numerator: Vec<&'static str>, denominator: Vec<&'static str>, neutral_nulls| {
            (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
                name,
                kind: PageOrderKind::CounterRatio {
                    numerator,
                    denominator,
                    neutral_nulls,
                },
            })
        };
    let one_over = |name, numerator: &[&str], denominator: &[&str]| {
        counters(
            name,
            vec![column(numerator)?],
            vec![column(denominator)?],
            false,
        )
    };
    let gauges = |name, numerator: &[&str], denominator: &[&str]| {
        let numerator = columns(numerator);
        let denominator = columns(denominator);
        (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
            name,
            kind: PageOrderKind::ValueRatio {
                numerator,
                denominator,
            },
        })
    };
    match token {
        "derived.mean_exec_ms_per_call" => one_over(
            "mean_exec_ms_per_call",
            &["total_exec_time", "total_time"],
            &["calls"],
        ),
        "derived.rows_per_call" => one_over("rows_per_call", &["rows"], &["calls"]),
        "derived.blocks_per_call" => counters(
            "blocks_per_call",
            [
                "shared_blks_hit",
                "shared_blks_read",
                "local_blks_hit",
                "local_blks_read",
            ]
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect(),
            vec![column(&["calls"])?],
            false,
        ),
        "derived.hit_pct" => {
            let hit = column(&["shared_blks_hit"])?;
            let read = column(&["shared_blks_read"])?;
            counters("hit_pct", vec![hit], vec![hit, read], false)
        }
        "derived.wal_per_call" => one_over("wal_per_call", &["wal_bytes"], &["calls"]),
        "derived.plan_time_pct" => {
            let planning = column(&["total_plan_time"])?;
            let execution = column(&["total_exec_time", "total_time"])?;
            counters(
                "plan_time_pct",
                vec![planning],
                vec![planning, execution],
                false,
            )
        }
        "derived.cv" => Some(PageOrder {
            name: "cv",
            kind: PageOrderKind::ValueRatio {
                numerator: vec![column(&["stddev_exec_time", "stddev_time"])?],
                denominator: vec![column(&["mean_exec_time", "mean_time"])?],
            },
        }),
        "derived.dead_pct" => gauges("dead_pct", &["n_dead_tup"], &["n_live_tup", "n_dead_tup"]),
        "derived.hot_pct" => one_over("hot_pct", &["n_tup_hot_upd"], &["n_tup_upd"]),
        "derived.new_page_pct" => one_over("new_page_pct", &["n_tup_newpage_upd"], &["n_tup_upd"]),
        "derived.sequential_share_pct" => counters(
            "sequential_share_pct",
            columns(&["seq_scan"]),
            columns(&["seq_scan", "idx_scan"]),
            true,
        ),
        "derived.seq_tuples_per_scan" => {
            one_over("seq_tuples_per_scan", &["seq_tup_read"], &["seq_scan"])
        }
        "derived.idx_tuples_per_scan" => {
            one_over("idx_tuples_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        "derived.buffer_hit_pct" => counters(
            "buffer_hit_pct",
            columns(&[
                "heap_blks_hit",
                "idx_blks_hit",
                "toast_blks_hit",
                "tidx_blks_hit",
            ]),
            columns(&[
                "heap_blks_hit",
                "heap_blks_read",
                "idx_blks_hit",
                "idx_blks_read",
                "toast_blks_hit",
                "toast_blks_read",
                "tidx_blks_hit",
                "tidx_blks_read",
            ]),
            true,
        ),
        "derived.vacuum_mean_ms" => {
            one_over("vacuum_mean_ms", &["total_vacuum_time"], &["vacuum_count"])
        }
        "derived.autovacuum_mean_ms" => one_over(
            "autovacuum_mean_ms",
            &["total_autovacuum_time"],
            &["autovacuum_count"],
        ),
        "derived.analyze_mean_ms" => one_over(
            "analyze_mean_ms",
            &["total_analyze_time"],
            &["analyze_count"],
        ),
        "derived.autoanalyze_mean_ms" => one_over(
            "autoanalyze_mean_ms",
            &["total_autoanalyze_time"],
            &["autoanalyze_count"],
        ),
        "derived.tuples_per_scan" => one_over("tuples_per_scan", &["idx_tup_read"], &["idx_scan"]),
        "derived.fetches_per_scan" => {
            one_over("fetches_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        _ => None,
    }
}

fn validate_search_projection(
    search: Option<&StructuredSearch>,
    sections: &[SectionPlans],
) -> Result<(), QueryError> {
    if let Some(search) = search {
        let [section] = sections else {
            return Err(QueryError::BadFilter("search".to_owned()));
        };
        if !section.plans.iter().any(|plan| {
            search.member_clauses().all(|clause| {
                !search_clause_columns(&section.logical_name, plan, clause.key).is_empty()
            })
        }) {
            return Err(QueryError::BadFilter("search".to_owned()));
        }
    }
    Ok(())
}

fn validate_exact_locator(
    segment: &Segment,
    request: &SnapshotRequest,
    sections: &[SectionPlans],
) -> Result<(), QueryError> {
    if let Some(ordinal) = request.row_ordinal {
        let [section] = sections else {
            return Err(QueryError::BadCursor);
        };
        let [plan] = section.plans.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let Some(timestamp) = plan.timestamp else {
            return Err(QueryError::BadCursor);
        };
        if ordinal >= plan.rows {
            return Err(QueryError::BadCursor);
        }
        let mut exact = false;
        segment.visit_rows(plan.type_id, &[timestamp], ordinal, 1, |_stored, row| {
            exact = row_timestamp(&row, timestamp) == Some(request.at);
            false
        })?;
        if !exact {
            return Err(QueryError::BadCursor);
        }
    }
    Ok(())
}

/// Keyed rendered process row. Locator fields identify the exact physical row;
/// `pid` and `ppid` appear both as typed members and in `fields`.
#[derive(Debug)]
pub struct ProcessRowOut {
    /// Process identifier.
    pub pid: i64,
    /// Parent process identifier, when recorded.
    pub ppid: Option<i64>,
    /// Exact source segment identifier.
    pub segment_id: i64,
    /// Exact physical layout identifier.
    pub type_id: u32,
    /// Exact physical row ordinal.
    pub row_ordinal: u64,
    /// Selected row timestamp.
    pub at: i64,
    /// Stable row identity.
    pub identity: crate::RowIdentity,
    /// Projected fields keyed by public name.
    pub fields: BTreeMap<String, Value>,
}

/// Keyed rendered `PostgreSQL` row with the exact physical locator used by row
/// detail.
#[derive(Debug)]
pub struct PlainRowOut {
    /// Exact source segment identifier.
    pub segment_id: i64,
    /// Exact physical layout identifier.
    pub type_id: u32,
    /// Exact physical row ordinal.
    pub row_ordinal: u64,
    /// Selected row timestamp.
    pub at: i64,
    /// Stable row identity.
    pub identity: crate::RowIdentity,
    /// Projected fields keyed by public name.
    pub fields: BTreeMap<String, Value>,
}

type RankedRecord<'a> = (&'a Plan, Value, crate::RowIdentity);
type RankedLocatorKey = (usize, i64, String);

impl PreparedSnapshot {
    pub(crate) const fn stability(&self) -> QueryStability {
        self.stability
    }

    pub(crate) fn validator_input(&self) -> Option<(&str, &str, &[DatasetSegment])> {
        (self.stability == QueryStability::Immutable).then_some((
            "snapshot",
            self.validator_shape.as_str(),
            self.validator_segments.as_slice(),
        ))
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let sink = std::cell::RefCell::new(sink);
        self.stream_with(&mut |bytes| sink.borrow_mut().record(bytes), &|| {
            sink.borrow().cancelled()
        })
    }

    fn stream_with(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        if cancelled()
            || !emit(record(json!({
                "record": "snapshot",
                "segment": { "id": self.anchor.id().to_string() },
                "at": self.at.to_string(),
            }))?)
        {
            return Ok(());
        }
        if self.first_match_query_id.is_some() {
            return self.emit_first_match(emit, cancelled);
        }
        if self.page_size.is_some() {
            if self.group.is_some() {
                return self.emit_relation_page(emit, cancelled);
            }
            return self.emit_page(emit, cancelled);
        }
        let mut facts = HashMap::new();
        for section in &self.sections {
            if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some() {
                if !self.emit_partitioned_section(section, emit, cancelled)? {
                    return Ok(());
                }
            } else {
                for (layout_index, plan) in section.plans.iter().enumerate() {
                    if cancelled() {
                        return Ok(());
                    }
                    if !self.emit_section(
                        section,
                        layout_index,
                        plan,
                        emit,
                        cancelled,
                        &mut facts,
                    )? {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn emit_partitioned_section(
        &self,
        section: &SectionPlans,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(false);
            }
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        for context in &contexts {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
                continue;
            }
            if cancelled() || !self.emit_context_rows(context, self.row_ordinal, emit, cancelled)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_context_rows(
        &self,
        context: &PageContext<'_>,
        row_ordinal: Option<u64>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let (start_row, row_count) = row_ordinal.map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let source = self.dataset.open(context.source)?;
        let process_users = ProcessUsers::load(&source, context.plan)?;
        let selection_dictionary = context.plan.exact_filter_dictionary(&source)?;
        #[cfg(test)]
        if context.plan.needs_selection_dictionary() {
            CONTEXT_SELECTION_DICTIONARIES.set(CONTEXT_SELECTION_DICTIONARIES.get() + 1);
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        let mut connected = true;
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                if context.window.matches(&row) {
                    chunk.push((ordinal, row));
                }
                if chunk.len() == SNAPSHOT_CHUNK_ROWS {
                    match self.emit_context_chunk(
                        context,
                        &source,
                        &process_users,
                        &selection_dictionary,
                        &mut chunk,
                        emit,
                        cancelled,
                    ) {
                        Ok(still_connected) => connected = still_connected,
                        Err(error) => failure = Some(error),
                    }
                }
                connected && failure.is_none() && !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if cancelled() || !connected {
            return Ok(false);
        }
        if chunk.is_empty() {
            return Ok(true);
        }
        self.emit_context_chunk(
            context,
            &source,
            &process_users,
            &selection_dictionary,
            &mut chunk,
            emit,
            cancelled,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "chunk emission carries the exact source, dictionaries, output sink, and cancellation boundary"
    )]
    fn emit_context_chunk(
        &self,
        context: &PageContext<'_>,
        source: &Segment,
        process_users: &ProcessUsers,
        selection_dictionary: &Dictionary,
        rows: &mut Vec<(u64, Row)>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        #[cfg(test)]
        CONTEXT_CHUNK_ROWS.set(CONTEXT_CHUNK_ROWS.get().max(rows.len()));
        let mut staged = Vec::new();
        for (ordinal, row) in rows.drain(..) {
            context
                .plan
                .validate_exact_filter_ids(&row, selection_dictionary)?;
            if !context.plan.matches(&row, selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(context.plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        #[cfg(test)]
        CONTEXT_STAGED_ROWS.set(CONTEXT_STAGED_ROWS.get().saturating_add(staged.len()));
        if staged.is_empty() {
            return Ok(true);
        }
        let dictionary = retained_dictionary(source, &staged)?;
        for staged in staged {
            let before = context.predecessor(&staged.row, &staged.identity);
            let value = Self::row_record(
                context.plan,
                &staged.row,
                before,
                context.elapsed_for(&staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                process_users,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_section(
        &self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<bool, QueryError> {
        if !Self::emit_layout(section, plan, emit)? {
            return Ok(false);
        }
        if !plan.applies() {
            return Ok(true);
        }
        let Some(timestamp) = plan.timestamp else {
            return self.emit_untimed(plan, emit, cancelled);
        };
        for context in
            self.timed_contexts(section, layout_index, plan, timestamp, cancelled, facts)?
        {
            if self.row_ordinal.is_some() && context.source.id() != self.anchor.id() {
                continue;
            }
            if cancelled()
                || !self.emit_context_rows(&context, self.row_ordinal, emit, cancelled)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_layout(
        section: &SectionPlans,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<bool, QueryError> {
        let fields = plan
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    field.column.and_then(|name| plan.contract.column(name)),
                )
            })
            .collect::<Vec<_>>();
        let mut layout = projected_layout(&section.logical_name, plan.contract, &fields);
        if section.logical_name == "os_process"
            && let Some(columns) = layout.get_mut("columns").and_then(Value::as_array_mut)
        {
            for column in columns {
                let Some(name) = column.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if name == CPU_TIME_VIRTUAL_FIELD {
                    *column = json!({
                        "name": name,
                        "type": "i64",
                        "class": "gauge",
                        "unit": "jiffies",
                        "nullable": true,
                        "available": true,
                    });
                } else if PROCESS_USER_VIRTUAL_FIELDS.contains(&name) {
                    *column = json!({
                        "name": name,
                        "type": "dictionary_value",
                        "class": "label",
                        "unit": "none",
                        "nullable": true,
                        "available": true,
                    });
                }
            }
        }
        Ok(emit(record(json!({
            "record": "layout",
            "layout": layout,
            "rates": output_rate_fields(plan),
        }))?))
    }

    fn emit_untimed(
        &self,
        plan: &Plan,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let (start_row, row_count) = self
            .row_ordinal
            .map_or((0, usize::MAX), |ordinal| (ordinal, 1));
        let rates = RateContext {
            previous: None,
            elapsed: None,
        };
        let mut rows = Vec::new();
        let source = self.dataset.open(&self.anchor)?;
        source.visit_rows(
            plan.type_id,
            &plan.projection,
            start_row,
            row_count,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                rows.push((ordinal, row));
                true
            },
        )?;
        if cancelled() {
            return Ok(false);
        }
        self.emit_rows(&source, plan, rows, rates, emit, cancelled)
    }

    fn emit_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        rows: Vec<(u64, Row)>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let selection_dictionary = plan.selection_dictionary(source, &rows)?;
        let mut staged = Vec::with_capacity(rows.len());
        for (ordinal, row) in rows {
            if !plan.matches(&row, &selection_dictionary) {
                continue;
            }
            let Some(identity) = identity_of(plan, &row) else {
                continue;
            };
            staged.push(StagedRow {
                ordinal,
                row,
                identity,
            });
        }
        self.emit_staged_rows(source, plan, staged, rates, emit, cancelled)
    }

    fn emit_staged_rows(
        &self,
        source: &Segment,
        plan: &Plan,
        staged: Vec<StagedRow>,
        rates: RateContext<'_>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let dictionary = retained_dictionary(source, &staged)?;
        let process_users = ProcessUsers::load(source, plan)?;
        for staged in staged {
            let before = rates
                .previous
                .and_then(|previous| previous.get(&staged.identity));
            let value = Self::row_record(
                plan,
                &staged.row,
                before,
                rates.elapsed,
                RowCoordinate {
                    segment_id: source.id(),
                    ordinal: staged.ordinal,
                },
                &dictionary,
                &process_users,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_page(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(());
            }
        }
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let process_users = contexts
            .iter()
            .map(|context| {
                let source = self.dataset.open(context.source)?;
                ProcessUsers::load(&source, context.plan)
                    .map(|users| (context.context_index, users))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let anchor = self
            .cursor
            .map(|cursor| self.cursor_anchor(&contexts, &process_users, cursor))
            .transpose()?;
        let page_size = self.page_size.ok_or(QueryError::BadCursor)?;
        let mut page = PageRows::new(page_size.saturating_add(1));
        let mut eligible = 0_u64;
        let mut excluded = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                anchor.as_ref(),
                &mut page,
                &mut eligible,
                &mut excluded,
                cancelled,
            )?;
            if cancelled() {
                return Ok(());
            }
        }
        let mut ranked = page.finish();
        let has_more = ranked.len() > page_size;
        let next_cursor = has_more.then(|| {
            let row = &ranked[page_size].staged;
            SnapshotCursor {
                segment_id: self.anchor.id(),
                active_position: self.anchor.active_position().unwrap_or(0),
                context_index: row.context_index,
                ordinal: row.ordinal,
                binding: self.binding,
            }
            .encode()
        });
        ranked.truncate(page_size);
        let returned = ranked.len();
        if !self.emit_page_rows(&contexts, &process_users, ranked, emit, cancelled)? {
            return Ok(());
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible,
                excluded,
                returned,
                has_more,
                next_cursor,
                page_size,
            },
            self.direction,
            emit,
        )?;
        Ok(())
    }

    /// Returns the first `limit` rows from the full sort and whether more
    /// matched. Supports `os_process` only.
    pub(crate) fn compute_process_rows(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<FinderResult<ProcessRowOut>, QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        if section.logical_name != "os_process" {
            return Err(QueryError::NoSuchSection);
        }
        let (records, truncated, as_of) = self.ranked_records(limit, cancelled)?;
        let rows = records
            .into_iter()
            .map(|(plan, record, identity)| process_row_out(plan, record, identity))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FinderResult {
            rows,
            truncated,
            as_of,
        })
    }

    /// Returns the first `limit` rows from the full sort for supported
    /// plain (non-relation) sections.
    pub(crate) fn compute_plain_rows(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<FinderResult<PlainRowOut>, QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        if !matches!(
            section.logical_name.as_str(),
            "pg_stat_activity"
                | "pg_locks"
                | "pg_stat_progress_vacuum"
                | "pg_stat_database"
                | "pg_stat_statements"
                | "pg_store_plans"
                | "pg_settings"
                | "instance_metadata"
        ) {
            return Err(QueryError::NoSuchSection);
        }
        let (records, truncated, as_of) = self.ranked_records(limit, cancelled)?;
        let rows = records
            .into_iter()
            .map(|(plan, record, identity)| plain_row_out(plan, record, identity))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FinderResult {
            rows,
            truncated,
            as_of,
        })
    }

    /// Ranks one section from the start and renders survivors through
    /// `row_record`. Callers validate the section name.
    fn ranked_records(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(Vec<RankedRecord<'_>>, bool, Option<i64>), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        let as_of = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let process_users = contexts
            .iter()
            .map(|context| {
                let source = self.dataset.open(context.source)?;
                ProcessUsers::load(&source, context.plan)
                    .map(|users| (context.context_index, users))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut page = PageRows::new(limit.saturating_add(1));
        let mut eligible = 0_u64;
        let mut excluded = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                None,
                &mut page,
                &mut eligible,
                &mut excluded,
                cancelled,
            )?;
            if cancelled() {
                return Err(QueryError::Cancelled);
            }
        }
        let mut ranked = page.finish();
        let has_more = ranked.len() > limit;
        ranked.truncate(limit);
        self.validate_ranked_locator_identities(&contexts, &ranked, cancelled)?;
        let records = self.render_ranked_rows(&contexts, &process_users, ranked)?;
        Ok((records, has_more, as_of))
    }

    /// Proves that every bounded finder survivor names one physical row.
    /// A second minimal projection keeps top-K retention bounded while finding
    /// a duplicate that appeared anywhere in the selected snapshot.
    fn validate_ranked_locator_identities(
        &self,
        contexts: &[PageContext<'_>],
        ranked: &[PageRankedRow],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let mut targets: HashMap<RankedLocatorKey, u8> = HashMap::with_capacity(ranked.len());
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let (at, identity) = encoded_locator_identity(context.plan, &ranked.staged.row)?;
            if targets
                .insert((context.context_index, at, identity), 0)
                .is_some()
            {
                return Err(non_unique_locator(context, at));
            }
        }
        for context in contexts {
            if !targets
                .keys()
                .any(|(context_index, _at, _identity)| *context_index == context.context_index)
            {
                continue;
            }
            let timestamp = context.plan.timestamp.ok_or(QueryError::BadCursor)?;
            let mut projection = vec![timestamp];
            projection.extend(crate::identity_columns(context.plan.contract));
            let source = self.dataset.open(context.source)?;
            let mut failure = None;
            #[cfg(test)]
            PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
            source.visit_rows(
                context.plan.type_id,
                &projection,
                0,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        return false;
                    }
                    if !context.window.matches(&row) {
                        return true;
                    }
                    let (at, identity) = match encoded_locator_identity(context.plan, &row) {
                        Ok(key) => key,
                        Err(error) => {
                            failure = Some(error);
                            return false;
                        }
                    };
                    let Some(count) = targets.get_mut(&(context.context_index, at, identity))
                    else {
                        return true;
                    };
                    *count = count.saturating_add(1);
                    if *count > 1 {
                        failure = Some(non_unique_locator(context, at));
                        return false;
                    }
                    true
                },
            )?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if targets.values().all(|count| *count == 1) {
            return Ok(());
        }
        Err(QueryError::BadLocator(
            "cannot emit detail_locator: a selected row identity was not found".to_owned(),
        ))
    }

    /// Renders ranked rows with the `Plan` needed to re-key positional fields.
    fn render_ranked_rows<'a>(
        &self,
        contexts: &[PageContext<'a>],
        process_users: &HashMap<usize, ProcessUsers>,
        ranked: Vec<PageRankedRow>,
    ) -> Result<Vec<RankedRecord<'a>>, QueryError> {
        let mut ids_by_context: HashMap<usize, HashSet<u64>> = HashMap::new();
        for ranked in &ranked {
            for (_name, value) in ranked.staged.row.iter() {
                if let Cell::StrId(id) = value {
                    ids_by_context
                        .entry(ranked.staged.context_index)
                        .or_default()
                        .insert(*id);
                }
            }
        }
        let dictionaries = contexts
            .iter()
            .map(|context| {
                let ids = ids_by_context
                    .get(&context.context_index)
                    .cloned()
                    .unwrap_or_default();
                let source = self.dataset.open(context.source)?;
                resolved_dictionary(&source, &ids)
                    .map(|dictionary| (context.context_index, dictionary))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut records = Vec::with_capacity(ranked.len());
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let dictionary = dictionaries
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?;
            let before = context.predecessor(&ranked.staged.row, &ranked.staged.identity);
            let record = Self::row_record(
                context.plan,
                &ranked.staged.row,
                before,
                context.elapsed_for(&ranked.staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: ranked.staged.ordinal,
                },
                dictionary,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                self.text,
            )?;
            let locator_identity = crate::identity(context.plan.type_id, &ranked.staged.row)
                .map_err(|error| row_locator_invalid(&error))?;
            records.push((context.plan, record, locator_identity));
        }
        Ok(records)
    }

    fn emit_first_match(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let wanted = self.first_match_query_id.ok_or(QueryError::BadCursor)?;
        for plan in &section.plans {
            if cancelled() || !Self::emit_layout(section, plan, emit)? {
                return Ok(());
            }
        }
        let contexts = self.first_match_contexts(section, cancelled)?;
        let mut returned = 0_usize;
        for context in &contexts {
            let Some((ordinal, row, dictionary)) =
                self.first_match_row(context, wanted, cancelled)?
            else {
                if cancelled() {
                    return Ok(());
                }
                continue;
            };
            let value = Self::row_record(
                context.plan,
                &row,
                None,
                None,
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal,
                },
                &dictionary,
                &ProcessUsers::default(),
                None,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(());
            }
            returned = 1;
            break;
        }
        Self::emit_page_trailer(
            section,
            &contexts,
            &PageMetadata {
                eligible: u64::from(returned != 0),
                excluded: 0,
                returned,
                has_more: false,
                next_cursor: None,
                page_size: 1,
            },
            self.direction,
            emit,
        )
    }

    fn first_match_row(
        &self,
        context: &PageContext<'_>,
        wanted: i64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<(u64, Row, Dictionary)>, QueryError> {
        let query_id = context
            .plan
            .contract
            .column("queryid")
            .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))?
            .name;
        let query = context
            .plan
            .contract
            .column("query")
            .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))?
            .name;
        let mut projection = vec![query_id, query];
        if let RowWindow::Shared { timestamp, .. } = context.window {
            projection.push(timestamp);
        }
        projection.sort_unstable();
        projection.dedup();

        let source = self.dataset.open(context.source)?;
        let mut offset = 0;
        while offset < context.rows && !cancelled() {
            let mut candidate = None;
            source.visit_rows(
                context.plan.type_id,
                &projection,
                offset,
                usize::MAX,
                |ordinal, row| {
                    #[cfg(test)]
                    FIRST_MATCH_ROWS.set(FIRST_MATCH_ROWS.get() + 1);
                    if context.window.matches(&row)
                        && matches!(row.get(query_id), Some(Cell::I64(stored)) if *stored == wanted)
                    {
                        candidate = Some((ordinal, row));
                        return false;
                    }
                    !cancelled()
                },
            )?;
            let Some((ordinal, row)) = candidate else {
                return Ok(None);
            };
            offset = ordinal.checked_add(1).unwrap_or(context.rows);
            let Some(Cell::StrId(id)) = row.get(query) else {
                continue;
            };
            let dictionary = resolved_dictionary(&source, &HashSet::from([*id]))?;
            let resolved = dictionary.resolve(*id).ok_or_else(|| {
                QueryError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                )))
            })?;
            if resolved.stored_bytes().is_empty() {
                continue;
            }
            return Ok(Some((ordinal, row, dictionary)));
        }
        Ok(None)
    }

    fn emit_page_rows(
        &self,
        contexts: &[PageContext<'_>],
        process_users: &HashMap<usize, ProcessUsers>,
        ranked: Vec<PageRankedRow>,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<bool, QueryError> {
        let mut ids_by_context: HashMap<usize, HashSet<u64>> = HashMap::new();
        for ranked in &ranked {
            for (_name, value) in ranked.staged.row.iter() {
                if let Cell::StrId(id) = value {
                    ids_by_context
                        .entry(ranked.staged.context_index)
                        .or_default()
                        .insert(*id);
                }
            }
        }
        let dictionaries = contexts
            .iter()
            .map(|context| {
                let ids = ids_by_context
                    .get(&context.context_index)
                    .cloned()
                    .unwrap_or_default();
                let source = self.dataset.open(context.source)?;
                resolved_dictionary(&source, &ids)
                    .map(|dictionary| (context.context_index, dictionary))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let dictionary = dictionaries
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?;
            let before = context.predecessor(&ranked.staged.row, &ranked.staged.identity);
            let value = Self::row_record(
                context.plan,
                &ranked.staged.row,
                before,
                context.elapsed_for(&ranked.staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: ranked.staged.ordinal,
                },
                dictionary,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                self.text,
            )?;
            if cancelled() || !emit(record(&value)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn emit_page_trailer(
        section: &SectionPlans,
        contexts: &[PageContext<'_>],
        metadata: &PageMetadata,
        direction: Order,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
    ) -> Result<(), QueryError> {
        let mut order_by = Vec::new();
        for context in contexts {
            if let Some(order) = &context.order
                && !order_by.contains(&order.name)
            {
                order_by.push(order.name);
            }
        }
        let from = contexts
            .iter()
            .filter_map(|context| context.sample_from)
            .min();
        let to = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let _connected = emit(record(json!({
            "record": "snapshot_page",
            "logical_name": section.logical_name,
            "eligible": metadata.eligible.to_string(),
            "excluded": metadata.excluded.to_string(),
            "returned": metadata.returned.to_string(),
            "has_more": metadata.has_more,
            "truncated": metadata.eligible > metadata.returned as u64,
            "next_cursor": metadata.next_cursor,
            "page_size": metadata.page_size,
            "order_by": order_by,
            "order_direction": direction.as_str(),
            "from": from.map(|value| value.to_string()),
            "to": to.map(|value| value.to_string()),
        }))?);
        Ok(())
    }

    fn page_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some() {
            return self.partitioned_contexts(section, cancelled);
        }
        let mut contexts = Vec::with_capacity(section.plans.len());
        let mut facts = HashMap::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                let facts = self.page_facts(&section.logical_name, &self.anchor, &mut facts)?;
                contexts.push(PageContext {
                    context_index: layout_index,
                    plan,
                    logical_name: &section.logical_name,
                    source: &self.anchor,
                    rows: rows_of(&self.anchor, plan.type_id).unwrap_or(0),
                    window: RowWindow::Untimed,
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: None,
                    order: page_order(&section.logical_name, plan, &self.by),
                    search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                        search_columns(&section.logical_name, plan, search)
                    }),
                    clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                    block_size: facts.block_size.value(),
                });
                continue;
            };
            contexts.extend(self.timed_contexts(
                section,
                layout_index,
                plan,
                timestamp,
                cancelled,
                &mut facts,
            )?);
        }
        Ok(contexts)
    }

    fn page_facts(
        &self,
        logical_name: &str,
        source_ref: &DatasetSegment,
        cached: &mut HashMap<i64, PageFacts>,
    ) -> Result<PageFacts, QueryError> {
        if !matches!(
            logical_name,
            "os_process" | "pg_stat_statements" | "pg_store_plans"
        ) {
            return Ok(PageFacts::default());
        }
        let mut facts = cached.get(&source_ref.id()).copied().unwrap_or_default();
        let present = if logical_name == "os_process" {
            facts.clock_ticks_per_second != CachedFact::Unknown
        } else {
            facts.block_size != CachedFact::Unknown
        };
        if present {
            return Ok(facts);
        }
        let source = self.dataset.open(source_ref)?;
        if logical_name == "os_process" {
            facts.clock_ticks_per_second =
                clock_ticks_per_second(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        } else {
            facts.block_size =
                postgres_block_size(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        }
        cached.insert(source_ref.id(), facts);
        Ok(facts)
    }

    fn first_match_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                return Err(QueryError::BadFilter("first_match".to_owned()));
            };
            let mut moments = Vec::new();
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moment) = Self::moments(&source, plan, timestamp, self.at, cancelled)? {
                    moments.push((source_index, moment.current));
                }
            }
            let Some(current) = moments.iter().map(|(_source, at)| *at).max() else {
                continue;
            };
            for (source_index, at) in moments {
                if at != current {
                    continue;
                }
                let source = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                contexts.push(PageContext {
                    context_index: timed_context_index(
                        layout_index,
                        source_index,
                        self.source_count(),
                    ),
                    plan,
                    logical_name: &section.logical_name,
                    source,
                    rows: rows_of(source, plan.type_id).unwrap_or(0),
                    window: RowWindow::Shared { timestamp, current },
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: Some(current),
                    order: None,
                    search_columns: Vec::new(),
                    clock_ticks_per_second: None,
                    block_size: None,
                });
            }
        }
        Ok(contexts)
    }

    fn timed_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        timestamp: &'static str,
        cancelled: &(impl Fn() -> bool + ?Sized),
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let Some(moments) = self.shared_moments(plan, timestamp, cancelled)? else {
            return Ok(Vec::new());
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        if let Some(before) = moments.previous {
            for source_index in (0..self.source_count()).rev() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_some() {
                    let source = self.dataset.open(source_ref)?;
                    previous.extend(Self::collect(
                        &source,
                        plan,
                        timestamp,
                        before,
                        &order_columns,
                        cancelled,
                    )?);
                }
            }
        }
        let previous = Arc::new(previous);
        let elapsed = moments
            .previous
            .and_then(|before| moments.current.checked_sub(before))
            .filter(|elapsed| *elapsed > 0);
        let mut contexts = Vec::new();
        for source_index in 0..self.source_count() {
            let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
            if rows_of(source_ref, plan.type_id).is_none() {
                continue;
            }
            let source = self.dataset.open(source_ref)?;
            if Self::moments(&source, plan, timestamp, moments.current, cancelled)?
                .is_none_or(|source_moments| source_moments.current != moments.current)
            {
                continue;
            }
            let facts = self.page_facts(&section.logical_name, source_ref, facts)?;
            contexts.push(PageContext {
                context_index: timed_context_index(layout_index, source_index, self.source_count()),
                plan,
                logical_name: &section.logical_name,
                source: source_ref,
                rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
                window: RowWindow::Shared {
                    timestamp,
                    current: moments.current,
                },
                previous: Some(Arc::clone(&previous)),
                elapsed,
                elapsed_by_partition: Arc::new(BTreeMap::new()),
                sample_from: moments.previous,
                sample_to: Some(moments.current),
                order: order.clone(),
                search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                    search_columns(&section.logical_name, plan, search)
                }),
                clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                block_size: facts.block_size.value(),
            });
        }
        Ok(contexts)
    }

    fn shared_moments(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<Moments>, QueryError> {
        let anchor = self.dataset.open(&self.anchor)?;
        let here = Self::moments(&anchor, plan, timestamp, self.at, cancelled)?;
        drop(anchor);
        let mut current = here.map(|moments| moments.current);
        if current.is_none() || !self.pin_current {
            for source_ref in &self.prior_sources {
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, self.at, cancelled)?
                {
                    current = Some(
                        current.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
        }
        let Some(current) = current else {
            return Ok(None);
        };
        if self.current_from.is_some_and(|from| current < from) {
            return Ok(None);
        }
        let mut previous = None;
        if let Some(before) = current.checked_sub(1) {
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, before, cancelled)? {
                    previous = Some(
                        previous.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
        }
        Ok(Some(Moments { current, previous }))
    }

    fn partitioned_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let Some(spec) = SnapshotViewSpec::for_logical_name(&section.logical_name) else {
            return Ok(Vec::new());
        };
        let selected = self.selected_partitions(section, cancelled);
        if cancelled() {
            return Ok(Vec::new());
        }
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || plan.timestamp.is_none() {
                continue;
            }
            let rate_state =
                self.partition_rate_state(section, layout_index, plan, &selected, spec, cancelled)?;
            for source in std::iter::once(PartitionSource::Current).chain(
                (0..self.relation_predecessors.len())
                    .rev()
                    .map(PartitionSource::Earlier),
            ) {
                if let Some(context) = self.partitioned_context(
                    section,
                    layout_index,
                    plan,
                    source,
                    &selected,
                    &rate_state,
                    spec,
                ) {
                    contexts.push(context);
                }
            }
        }
        Ok(contexts)
    }

    fn selected_partitions(
        &self,
        section: &SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> BTreeMap<IdentityCell, SelectedPartition> {
        let source_by_id = std::iter::once((self.anchor.id(), PartitionSource::Current))
            .chain(
                self.relation_predecessors
                    .iter()
                    .enumerate()
                    .map(|(index, source)| (source.id(), PartitionSource::Earlier(index))),
            )
            .collect::<HashMap<_, _>>();
        let mut selected = BTreeMap::<IdentityCell, SelectedPartition>::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            if plan.timestamp.is_none() {
                continue;
            }
            for ((type_id, partition), retained) in &self.relation_moments {
                if *type_id != plan.type_id {
                    continue;
                }
                let Some(current) = retained
                    .current
                    .as_ref()
                    .and_then(|moment| located_moment(moment, &source_by_id))
                else {
                    continue;
                };
                if self.current_from.is_some_and(|from| current.at < from) {
                    continue;
                }
                let moments = PartitionMoments {
                    current,
                    previous: retained
                        .previous
                        .as_ref()
                        .and_then(|moment| located_moment(moment, &source_by_id)),
                };
                let candidate = SelectedPartition {
                    layout_index,
                    type_id: plan.type_id,
                    moments,
                };
                let replace = selected.get(partition).is_none_or(|chosen| {
                    (candidate.moments.current.at, candidate.type_id)
                        > (chosen.moments.current.at, chosen.type_id)
                });
                if replace {
                    selected.insert(partition.clone(), candidate);
                }
            }
        }
        selected
    }

    fn partition_rate_state(
        &self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &Plan,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        spec: SnapshotViewSpec,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<PartitionRateState, QueryError> {
        let Some(timestamp) = plan.timestamp else {
            return Err(QueryError::BadCursor);
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        let mut elapsed_by_partition = BTreeMap::new();
        let mut sample_from: Option<i64> = None;
        let mut sample_to: Option<i64> = None;
        let mut before_by_source =
            BTreeMap::<i64, (&DatasetSegment, BTreeMap<IdentityCell, i64>)>::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index {
                continue;
            }
            sample_to = Some(sample_to.map_or(selection.moments.current.at, |chosen| {
                chosen.max(selection.moments.current.at)
            }));
            let Some(before) = selection.moments.previous.as_ref() else {
                continue;
            };
            let Some(elapsed) = selection
                .moments
                .current
                .at
                .checked_sub(before.at)
                .filter(|elapsed| *elapsed > 0)
            else {
                continue;
            };
            for source in &before.sources {
                if let Some(reference) = self.partition_source(*source) {
                    before_by_source
                        .entry(reference.id())
                        .or_insert_with(|| (reference, BTreeMap::new()))
                        .1
                        .insert(partition.clone(), before.at);
                }
            }
            elapsed_by_partition.insert(partition.clone(), elapsed);
            sample_from = Some(sample_from.map_or(before.at, |chosen| chosen.min(before.at)));
        }
        for (_segment_id, (before_source_ref, partitions)) in before_by_source {
            let before_source = self.dataset.open(before_source_ref)?;
            previous.extend(Self::collect_partitions(
                &before_source,
                plan,
                timestamp,
                spec.temporal_partition,
                &partitions,
                &order_columns,
                cancelled,
            )?);
        }
        Ok(PartitionRateState {
            previous: Arc::new(previous),
            elapsed_by_partition: Arc::new(elapsed_by_partition),
            sample_from,
            sample_to,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the selected layout, source, and partition contract define one context"
    )]
    fn partitioned_context<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        source_kind: PartitionSource,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        rate_state: &PartitionRateState,
        spec: SnapshotViewSpec,
    ) -> Option<PageContext<'a>> {
        let timestamp = plan.timestamp?;
        if !selected.values().any(|selection| {
            selection.layout_index == layout_index
                && selection.moments.current.sources.contains(&source_kind)
        }) {
            return None;
        }
        let source_ref = self.partition_source(source_kind)?;
        let order = page_order(&section.logical_name, plan, &self.by);
        let mut current = BTreeMap::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index
                || !selection.moments.current.sources.contains(&source_kind)
            {
                continue;
            }
            current.insert(partition.clone(), selection.moments.current.at);
        }
        Some(PageContext {
            context_index: partition_context_index(
                layout_index,
                source_kind,
                self.relation_predecessors.len(),
            ),
            plan,
            logical_name: &section.logical_name,
            source: source_ref,
            rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
            window: RowWindow::Partitioned {
                timestamp,
                column: spec.temporal_partition,
                current,
            },
            previous: Some(Arc::clone(&rate_state.previous)),
            elapsed: None,
            elapsed_by_partition: Arc::clone(&rate_state.elapsed_by_partition),
            sample_from: rate_state.sample_from,
            sample_to: rate_state.sample_to,
            order,
            search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                search_columns(&section.logical_name, plan, search)
            }),
            clock_ticks_per_second: None,
            block_size: None,
        })
    }

    const fn source_count(&self) -> usize {
        self.prior_sources.len() + 1
    }

    fn source_for(&self, source_index: usize) -> Option<&DatasetSegment> {
        if source_index == 0 {
            Some(&self.anchor)
        } else {
            self.prior_sources.get(source_index - 1)
        }
    }

    fn partition_source(&self, source: PartitionSource) -> Option<&DatasetSegment> {
        match source {
            PartitionSource::Current => Some(&self.anchor),
            PartitionSource::Earlier(index) => self.relation_predecessors.get(index),
        }
    }

    fn collect_partitions(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        partition_column: &'static str,
        partitions: &BTreeMap<IdentityCell, i64>,
        extra_columns: &[&'static str],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Readings, QueryError> {
        #[cfg(test)]
        PARTITION_PREDECESSOR_VISITS.set(PARTITION_PREDECESSOR_VISITS.get().saturating_add(1));
        let mut collected = BTreeMap::new();
        let mut counters = projected_rate_columns(plan);
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.extend([timestamp, partition_column]);
        projection.sort_unstable();
        projection.dedup();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            let Some(partition) = row.get(partition_column).map(identity_cell) else {
                return true;
            };
            let Some(at) = partitions.get(&partition) else {
                return true;
            };
            if row_timestamp(&row, timestamp) != Some(*at) {
                return true;
            }
            let Some(key) = identity_of(plan, &row) else {
                return true;
            };
            let mut stored = BTreeMap::new();
            for name in &counters {
                if let Some(value) = row.get(name) {
                    stored.insert(*name, value.clone());
                }
            }
            collected.insert(key, stored);
            true
        })?;
        Ok(collected)
    }

    fn cursor_anchor(
        &self,
        contexts: &[PageContext<'_>],
        process_users: &HashMap<usize, ProcessUsers>,
        cursor: SnapshotCursor,
    ) -> Result<PageRankedRow, QueryError> {
        let context = contexts
            .iter()
            .find(|context| context.context_index == cursor.context_index)
            .ok_or(QueryError::BadCursor)?;
        if cursor.ordinal >= context.rows {
            return Err(QueryError::BadCursor);
        }
        let source = self.dataset.open(context.source)?;
        let mut stored = None;
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            cursor.ordinal,
            1,
            |ordinal, row| {
                stored = Some((ordinal, row));
                false
            },
        )?;
        let (ordinal, row) = stored.ok_or(QueryError::BadCursor)?;
        let dictionary = page_dictionary(
            &source,
            context,
            std::slice::from_ref(&(ordinal, row.clone())),
        )?;
        self.page_candidate(
            context,
            process_users
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?,
            ordinal,
            row,
            &dictionary,
        )
        .ok_or(QueryError::BadCursor)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one scan keeps page, counters and scope explicit"
    )]
    fn scan_page(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let source = self.dataset.open(context.source)?;
        // The collector's own statements are classified once per layout.
        let collector = if self.scope.filters(context.logical_name) {
            Some(CollectorStatements::scan(&source)?)
        } else {
            None
        };
        if !context.plan.needs_selection_dictionary()
            && context.search_columns.is_empty()
            && context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan))
                .is_none()
        {
            #[cfg(test)]
            PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
            let dictionary = Dictionary::default();
            source.visit_rows(
                context.plan.type_id,
                &context.plan.projection,
                0,
                usize::MAX,
                |ordinal, row| {
                    self.rank_page_row(
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        excluded,
                        collector.as_ref(),
                        &dictionary,
                        ordinal,
                        row,
                    );
                    !cancelled()
                },
            )?;
            return Ok(());
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        #[cfg(test)]
        PAGE_CHUNK_ROWS.set(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        #[cfg(test)]
        PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                chunk.push((ordinal, row));
                if chunk.len() == SNAPSHOT_CHUNK_ROWS
                    && let Err(error) = self.rank_page_chunk(
                        &source,
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        excluded,
                        collector.as_ref(),
                        &mut chunk,
                    )
                {
                    failure = Some(error);
                    return false;
                }
                !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if !cancelled() && !chunk.is_empty() {
            self.rank_page_chunk(
                &source,
                context,
                process_users,
                anchor,
                page,
                eligible,
                excluded,
                collector.as_ref(),
                &mut chunk,
            )?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "page ranking keeps segment-local user resolution beside existing paging state"
    )]
    fn rank_page_chunk(
        &self,
        source: &Segment,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        collector: Option<&CollectorStatements>,
        chunk: &mut Vec<(u64, Row)>,
    ) -> Result<(), QueryError> {
        let dictionary = page_dictionary(source, context, chunk)?;
        for (ordinal, row) in chunk.drain(..) {
            self.rank_page_row(
                context,
                process_users,
                anchor,
                page,
                eligible,
                excluded,
                collector,
                &dictionary,
                ordinal,
                row,
            );
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "ranking keeps request, page, and physical-row coordinates explicit"
    )]
    fn rank_page_row(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        collector: Option<&CollectorStatements>,
        dictionary: &Dictionary,
        ordinal: u64,
        row: Row,
    ) {
        let Some(candidate) = self.page_candidate(context, process_users, ordinal, row, dictionary)
        else {
            return;
        };
        // A collector statement that would otherwise be listed is counted, not shown.
        if collector.is_some_and(|statements| statements.excludes(&candidate.staged.row)) {
            *excluded = excluded.saturating_add(1);
            return;
        }
        *eligible = eligible.saturating_add(1);
        if anchor.is_none_or(|anchor| candidate.cmp(anchor) != Ordering::Greater) {
            page.push(candidate);
        }
    }

    fn page_candidate(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        ordinal: u64,
        row: Row,
        dictionary: &Dictionary,
    ) -> Option<PageRankedRow> {
        if !context.window.matches(&row) || !context.plan.matches(&row, dictionary) {
            return None;
        }
        let identity = identity_of(context.plan, &row)?;
        if self.search.as_ref().is_some_and(|search| {
            !search.matches_all(|clause| {
                if matches!(clause.value, SearchValue::Quantity(_)) {
                    result_search_matches(context, &row, &identity, clause)
                } else {
                    search_clause_matches(
                        context.logical_name,
                        context.plan,
                        &row,
                        dictionary,
                        Some(process_users),
                        clause,
                    )
                }
            })
        }) {
            return None;
        }
        let value = page_order_value(context, &row, &identity, dictionary);
        Some(PageRankedRow {
            staged: PageStagedRow {
                context_index: context.context_index,
                ordinal,
                row,
                identity,
            },
            value,
            direction: self.direction,
        })
    }

    fn moments(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<Moments>, QueryError> {
        let mut current: Option<i64> = None;
        let mut previous: Option<i64> = None;
        segment.visit_rows(
            plan.type_id,
            &[timestamp],
            0,
            usize::MAX,
            |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let Some(stored) = row_timestamp(&row, timestamp) else {
                    return true;
                };
                if stored > at {
                    return true;
                }
                match current {
                    Some(chosen) if stored == chosen => {}
                    Some(chosen) if stored > chosen => {
                        previous = Some(chosen);
                        current = Some(stored);
                    }
                    Some(chosen)
                        if previous.is_none_or(|before| stored > before) && stored < chosen =>
                    {
                        previous = Some(stored);
                    }
                    Some(_) => {}
                    None => current = Some(stored),
                }
                true
            },
        )?;
        Ok(current.map(|current| Moments { current, previous }))
    }

    fn collect(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        at: i64,
        extra_columns: &[&'static str],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Readings, QueryError> {
        let mut collected = BTreeMap::new();
        let counters = projected_rate_columns(plan);
        let mut counters = counters;
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.push(timestamp);
        projection.sort_unstable();
        projection.dedup();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(at) {
                return true;
            }
            let Some(key) = identity_of(plan, &row) else {
                return true;
            };
            let stored = counters
                .iter()
                .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
                .collect();
            collected.insert(key, stored);
            true
        })?;
        if cancelled() {
            collected.clear();
            return Ok(collected);
        }
        Ok(collected)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one row renderer combines physical values, rates, coordinates, and segment-local reference data"
    )]
    fn row_record(
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        coordinate: RowCoordinate,
        dictionary: &Dictionary,
        process_users: &ProcessUsers,
        text_limit: Option<u64>,
    ) -> Result<Value, QueryError> {
        let stamped = plan.timestamp.and_then(|column| row_timestamp(row, column));
        let mut values = Vec::with_capacity(plan.fields.len());
        for field in &plan.fields {
            let Some(column) = field.column else {
                if field.name == CPU_TIME_VIRTUAL_FIELD {
                    values.push(scheduled_ticks(row));
                    continue;
                }
                let uid_column = match field.name.as_str() {
                    "user" => Some("uid"),
                    "effective_user" => Some("euid"),
                    _ => None,
                };
                values.push(
                    uid_column
                        .and_then(|column| process_users.for_row(row, column))
                        .map_or(Value::Null, |name| Value::String(name.to_owned())),
                );
                continue;
            };
            let stored = row.get(column);
            let is_rate = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            let exact_plan_calls =
                matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001) && field.name == "calls";
            if is_rate && !exact_plan_calls {
                values.push(rate(stored, before, column, elapsed));
                continue;
            }
            let rendered = match stored {
                Some(stored) => cell(stored, dictionary)?,
                None => Value::Null,
            };
            values.push(match text_limit {
                Some(limit) => shorten(rendered, limit),
                None => rendered,
            });
        }
        Ok(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": coordinate.ordinal.to_string(),
            "segment_id": coordinate.segment_id.to_string(),
            "timestamp": stamped.map(|stored| stored.to_string()),
            "values": values,
        }))
    }
}

/// Locator and positional values extracted from `row_record` output.
struct RowLocator {
    segment_id: i64,
    row_ordinal: u64,
    at: i64,
    values: Vec<Value>,
}

fn row_locator_invalid(message: &str) -> QueryError {
    QueryError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.to_owned(),
    )))
}

fn row_locator(plan: &Plan, mut record: Value) -> Result<RowLocator, QueryError> {
    let object = record
        .as_object_mut()
        .ok_or_else(|| row_locator_invalid("row record is not an object"))?;
    let Some(Value::Array(values)) = object.remove("values") else {
        return Err(row_locator_invalid("row record has no values array"));
    };
    if values.len() != plan.fields.len() {
        return Err(row_locator_invalid(
            "row field count does not match its rendered values",
        ));
    }
    let segment_id = object
        .remove("segment_id")
        .and_then(|value| value.as_str()?.parse::<i64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no segment_id"))?;
    let row_ordinal = object
        .remove("ordinal")
        .and_then(|value| value.as_str()?.parse::<u64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no ordinal"))?;
    let at = object
        .remove("timestamp")
        .and_then(|value| value.as_str()?.parse::<i64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no timestamp"))?;
    Ok(RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    })
}

/// Re-keys a rendered process row. A missing `pid` is invalid rendered data.
fn process_row_out(
    plan: &Plan,
    record: Value,
    identity: crate::RowIdentity,
) -> Result<ProcessRowOut, QueryError> {
    let RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    } = row_locator(plan, record)?;
    let mut fields = BTreeMap::new();
    let mut pid = None;
    let mut parent = None;
    for (field, value) in plan.fields.iter().zip(values) {
        match field.name.as_str() {
            "pid" => pid = value.as_i64(),
            "ppid" => parent = value.as_i64(),
            _ => {}
        }
        fields.insert(field.name.clone(), value);
    }
    let pid = pid.ok_or_else(|| row_locator_invalid("process row has no pid"))?;
    Ok(ProcessRowOut {
        pid,
        ppid: parent,
        segment_id,
        type_id: plan.type_id,
        row_ordinal,
        at,
        identity,
        fields,
    })
}

fn plain_row_out(
    plan: &Plan,
    record: Value,
    identity: crate::RowIdentity,
) -> Result<PlainRowOut, QueryError> {
    let RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    } = row_locator(plan, record)?;
    let mut fields = BTreeMap::new();
    for (field, value) in plan.fields.iter().zip(values) {
        fields.insert(field.name.clone(), value);
    }
    if matches!(
        logical_section_name(plan.type_id),
        Some("pg_stat_statements" | "pg_store_plans")
    ) {
        fields.extend(derived_ratio_fields(&fields));
    }
    Ok(PlainRowOut {
        segment_id,
        type_id: plan.type_id,
        row_ordinal,
        at,
        identity,
        fields,
    })
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn ratio_field(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| fields.get(*name))
        .and_then(value_as_f64)
}

fn ratio_sum(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    for name in names {
        total += ratio_field(fields, &[name])?;
    }
    Some(total)
}

fn ratio_value(numerator: Option<f64>, denominator: Option<f64>) -> Value {
    match numerator.zip(denominator).map(|(n, d)| n / d) {
        Some(value) if value.is_finite() => json!(value),
        _ => Value::Null,
    }
}

fn derived_ratio_fields(fields: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let calls = ratio_field(fields, &["calls_per_second", "calls"]);
    let execution = ratio_field(fields, &["total_exec_time", "total_time"]);
    let hit = ratio_field(fields, &["shared_blks_hit"]);
    let read = ratio_field(fields, &["shared_blks_read"]);
    let planning = ratio_field(fields, &["total_plan_time"]);
    let blocks = ratio_sum(
        fields,
        &[
            "shared_blks_hit",
            "shared_blks_read",
            "local_blks_hit",
            "local_blks_read",
        ],
    );

    BTreeMap::from([
        (
            "derived_mean_exec_ms_per_call".to_owned(),
            ratio_value(execution, calls),
        ),
        (
            "derived_rows_per_call".to_owned(),
            ratio_value(ratio_field(fields, &["rows"]), calls),
        ),
        (
            "derived_blocks_per_call".to_owned(),
            ratio_value(blocks, calls),
        ),
        (
            "derived_hit_fraction".to_owned(),
            ratio_value(hit, hit.zip(read).map(|(hit, read)| hit + read)),
        ),
        (
            "derived_wal_per_call".to_owned(),
            ratio_value(ratio_field(fields, &["wal_bytes"]), calls),
        ),
        (
            "derived_plan_time_fraction".to_owned(),
            ratio_value(
                planning,
                planning
                    .zip(execution)
                    .map(|(planning, execution)| planning + execution),
            ),
        ),
        (
            "derived_cv".to_owned(),
            ratio_value(
                ratio_field(fields, &["stddev_exec_time", "stddev_time"]),
                ratio_field(fields, &["mean_exec_time", "mean_time"]),
            ),
        ),
    ])
}

impl PageRows {
    const fn new(limit: usize) -> Self {
        Self {
            limit,
            rows: BinaryHeap::new(),
        }
    }

    fn push(&mut self, row: PageRankedRow) {
        if self.limit == 0 {
            return;
        }
        if self.rows.len() < self.limit {
            self.rows.push(Reverse(row));
            return;
        }
        let Some(worst) = self.rows.peek() else {
            return;
        };
        if row > worst.0 {
            self.rows.pop();
            self.rows.push(Reverse(row));
        }
    }

    fn finish(self) -> Vec<PageRankedRow> {
        let mut rows: Vec<PageRankedRow> = self.rows.into_iter().map(|Reverse(row)| row).collect();
        rows.sort_by(|left, right| right.cmp(left));
        rows
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.rows.len()
    }
}

impl PartialEq for PageRankedRow {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PageRankedRow {}

impl PartialOrd for PageRankedRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PageRankedRow {
    fn cmp(&self, other: &Self) -> Ordering {
        debug_assert_eq!(
            self.direction, other.direction,
            "one page heap cannot compare opposite sort directions"
        );
        compare_page_order_values(self.value.as_ref(), other.value.as_ref(), self.direction)
            .then_with(|| other.staged.context_index.cmp(&self.staged.context_index))
            .then_with(|| other.staged.ordinal.cmp(&self.staged.ordinal))
    }
}

fn compare_page_order_values(
    left: Option<&PageOrderValue>,
    right: Option<&PageOrderValue>,
    direction: Order,
) -> Ordering {
    let ordered = match (left, right) {
        (Some(PageOrderValue::Integer(left)), Some(PageOrderValue::Integer(right))) => {
            left.cmp(right)
        }
        (Some(PageOrderValue::Float(left)), Some(PageOrderValue::Float(right)))
        | (Some(PageOrderValue::FloatRate(left)), Some(PageOrderValue::FloatRate(right)))
        | (Some(PageOrderValue::FloatRatio(left)), Some(PageOrderValue::FloatRatio(right))) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (
            Some(PageOrderValue::IntegerRate {
                delta: left,
                elapsed: left_elapsed,
            }),
            Some(PageOrderValue::IntegerRate {
                delta: right,
                elapsed: right_elapsed,
            }),
        ) => (left * i128::from(*right_elapsed)).cmp(&(right * i128::from(*left_elapsed))),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator: left_numerator,
                denominator: left_denominator,
            }),
            Some(PageOrderValue::IntegerRatio {
                numerator: right_numerator,
                denominator: right_denominator,
            }),
        ) => compare_u128_ratios(
            *left_numerator,
            *left_denominator,
            *right_numerator,
            *right_denominator,
        ),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
            Some(PageOrderValue::FloatRatio(right)),
        ) => integer_ratio_as_f64(*numerator, *denominator)
            .partial_cmp(right)
            .unwrap_or(Ordering::Equal),
        (
            Some(PageOrderValue::FloatRatio(left)),
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
        ) => left
            .partial_cmp(&integer_ratio_as_f64(*numerator, *denominator))
            .unwrap_or(Ordering::Equal),
        (Some(PageOrderValue::Text(left)), Some(PageOrderValue::Text(right))) => left.cmp(right),
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
    };
    match direction {
        Order::Asc => ordered.reverse(),
        Order::Desc => ordered,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "this path compares an exact ratio with an already inexact floating source"
)]
fn integer_ratio_as_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

fn compare_u128_ratios(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reverse = false;
    loop {
        let whole = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if whole != Ordering::Equal {
            return if reverse { whole.reverse() } else { whole };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        let ended = match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (false, false) => None,
        };
        if let Some(ended) = ended {
            return if reverse { ended.reverse() } else { ended };
        }
        (left_numerator, left_denominator) = (left_denominator, left_remainder);
        (right_numerator, right_denominator) = (right_denominator, right_remainder);
        reverse = !reverse;
    }
}

#[cfg(test)]
fn compare_ordered(left: Option<OrderedNumber>, right: Option<OrderedNumber>) -> Ordering {
    match (left, right) {
        (Some(OrderedNumber::Integer(left)), Some(OrderedNumber::Integer(right))) => {
            left.cmp(&right)
        }
        (Some(OrderedNumber::Float(left)), Some(OrderedNumber::Float(right))) => {
            left.partial_cmp(&right).unwrap_or(Ordering::Equal)
        }
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
    }
}

fn ordered_cell(cell: &Cell) -> Option<OrderedNumber> {
    match cell {
        Cell::I16(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I64(value) | Cell::Ts(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U64(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(value) if value.is_finite() => Some(OrderedNumber::Float(*value)),
        Cell::Bool(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(_) | Cell::StrId(_) | Cell::ListI32(_) | Cell::Null => None,
    }
}

fn page_dictionary(
    segment: &Segment,
    context: &PageContext<'_>,
    rows: &[(u64, Row)],
) -> Result<Dictionary, QueryError> {
    let mut ids = HashSet::new();
    for (_ordinal, row) in rows {
        if !context.window.matches(row) {
            continue;
        }
        context.plan.add_selection_ids(row, &mut ids);
        for column in context.search_columns.iter().copied().chain(
            context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan)),
        ) {
            if let Some(Cell::StrId(id)) = row.get(column) {
                ids.insert(*id);
            }
        }
    }
    #[cfg(test)]
    if !ids.is_empty() {
        PAGE_CANDIDATE_DICTIONARIES.set(PAGE_CANDIDATE_DICTIONARIES.get() + 1);
    }
    resolved_dictionary(segment, &ids)
}

fn page_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
) -> Option<PageOrderValue> {
    let order = context.order.as_ref()?;
    match &order.kind {
        PageOrderKind::Column(column) => {
            column_order_value(context, row, identity, dictionary, column)
        }
        PageOrderKind::CounterRatio {
            numerator,
            denominator,
            neutral_nulls,
        } => {
            let _elapsed = context.elapsed_for(row)?;
            let before = context.predecessor(row, identity)?;
            let numerator = counter_sum(row, before, numerator, *neutral_nulls)?;
            let denominator = counter_sum(row, before, denominator, *neutral_nulls)?;
            ratio_order_value(numerator, denominator)
        }
        PageOrderKind::ValueRatio {
            numerator,
            denominator,
        } => ratio_order_value(value_sum(row, numerator)?, value_sum(row, denominator)?),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "an interval of 2^52 microseconds is 142 years"
)]
fn column_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
    column: &'static str,
) -> Option<PageOrderValue> {
    let stored = row.get(column)?;
    let cumulative = context
        .plan
        .contract
        .column(column)
        .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
    if cumulative {
        let elapsed = context.elapsed_for(row)?;
        let earlier = context.predecessor(row, identity)?.get(column)?;
        return match counter_delta(stored, earlier)? {
            OrderedNumber::Integer(delta) => Some(PageOrderValue::IntegerRate { delta, elapsed }),
            OrderedNumber::Float(delta) => {
                let seconds = elapsed as f64 / 1_000_000.0;
                let rate = delta / seconds;
                rate.is_finite().then_some(PageOrderValue::FloatRate(rate))
            }
        };
    }
    match stored {
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .map(Resolved::stored_bytes)
            .map(<[u8]>::to_vec)
            .map(PageOrderValue::Text),
        _ => match ordered_cell(stored)? {
            OrderedNumber::Integer(value) => Some(PageOrderValue::Integer(value)),
            OrderedNumber::Float(value) => Some(PageOrderValue::Float(value)),
        },
    }
}

fn counter_sum(
    row: &Row,
    before: &CounterReadings,
    columns: &[&'static str],
    neutral_nulls: bool,
) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let (Some(now), Some(earlier)) = (row.get(column), before.get(column)) else {
            return None;
        };
        if neutral_nulls && matches!((now, earlier), (Cell::Null, Cell::Null)) {
            continue;
        }
        let value = counter_delta(now, earlier)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

fn value_sum(row: &Row, columns: &[&'static str]) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let value = row.get(column).and_then(ordered_cell)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

fn add_ordered(left: Option<OrderedNumber>, right: OrderedNumber) -> Option<OrderedNumber> {
    match (left, right) {
        (None, right) => Some(right),
        (Some(OrderedNumber::Integer(left)), OrderedNumber::Integer(right)) => {
            left.checked_add(right).map(OrderedNumber::Integer)
        }
        (Some(left), right) => {
            let sum = left.as_f64() + right.as_f64();
            sum.is_finite().then_some(OrderedNumber::Float(sum))
        }
    }
}

fn ratio_order_value(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
) -> Option<PageOrderValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(PageOrderValue::IntegerRatio {
                numerator: u128::try_from(numerator).ok()?,
                denominator: u128::try_from(denominator).ok()?,
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator;
            (denominator > 0.0 && ratio.is_finite()).then_some(PageOrderValue::FloatRatio(ratio))
        }
    }
}

fn search_columns(logical_name: &str, plan: &Plan, search: &StructuredSearch) -> Vec<&'static str> {
    let mut columns = Vec::new();
    for clause in &search.clauses {
        let mut wanted = search_clause_columns(logical_name, plan, clause.key);
        if let Some((_clause, field)) = search
            .result_clauses(logical_name)
            .find(|(candidate, _field)| candidate.key == clause.key)
        {
            wanted.extend(
                field
                    .dependencies
                    .iter()
                    .filter_map(|name| plan.contract.column(name).map(|column| column.name)),
            );
        }
        for column in wanted {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
    }
    columns
}

fn clock_ticks_per_second(segment: &Segment) -> Result<Option<u128>, QueryError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("instance_metadata") {
            continue;
        }
        let Some(column) =
            contract(type_id).and_then(|layout| layout.column("clock_ticks_per_sec"))
        else {
            continue;
        };
        let mut stored = None;
        segment.visit_rows(type_id, &[column.name], 0, 1, |_ordinal, row| {
            stored = row
                .get(column.name)
                .and_then(ordered_cell)
                .and_then(|value| {
                    let OrderedNumber::Integer(value) = value else {
                        return None;
                    };
                    u128::try_from(value).ok().filter(|value| *value > 0)
                });
            false
        })?;
        if stored.is_some() {
            return Ok(stored);
        }
    }
    Ok(None)
}

fn postgres_block_size(segment: &Segment) -> Result<Option<u128>, QueryError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("pg_settings") {
            continue;
        }
        let Some(layout) = contract(type_id) else {
            continue;
        };
        let (Some(name), Some(setting)) = (layout.column("name"), layout.column("setting")) else {
            continue;
        };
        let mut candidates = Vec::new();
        let mut ids = HashSet::new();
        segment.visit_rows(
            type_id,
            &[name.name, setting.name],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(Cell::StrId(name_id)), Some(Cell::StrId(setting_id))) =
                    (row.get(name.name), row.get(setting.name))
                else {
                    return true;
                };
                ids.insert(*name_id);
                ids.insert(*setting_id);
                candidates.push((*name_id, *setting_id));
                true
            },
        )?;
        let dictionary = resolved_dictionary(segment, &ids)?;
        for (name_id, setting_id) in candidates {
            let Some(Resolved::Str(name)) = dictionary.resolve(name_id) else {
                continue;
            };
            if name != b"block_size" {
                continue;
            }
            let Some(Resolved::Str(setting)) = dictionary.resolve(setting_id) else {
                continue;
            };
            let Ok(setting) = std::str::from_utf8(setting) else {
                continue;
            };
            if let Ok(value) = setting.parse::<u128>()
                && value > 0
            {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

#[expect(
    variant_size_differences,
    reason = "exact quantity factors stay inline so per-row comparisons do not allocate"
)]
enum SearchMetricValue {
    Exact {
        numerator: ProductFactors,
        denominator: ProductFactors,
    },
    Float(f64),
}

#[derive(Clone, Copy)]
struct ProductFactors {
    values: [u128; 4],
    len: usize,
}

impl ProductFactors {
    const fn one(value: u128) -> Self {
        Self {
            values: [value, 1, 1, 1],
            len: 1,
        }
    }

    const fn two(first: u128, second: u128) -> Self {
        Self {
            values: [first, second, 1, 1],
            len: 2,
        }
    }

    fn push(mut self, value: u128) -> Option<Self> {
        let slot = self.values.get_mut(self.len)?;
        *slot = value;
        self.len += 1;
        Some(self)
    }

    fn as_slice(&self) -> &[u128] {
        &self.values[..self.len]
    }
}

impl SearchMetricValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics must be scaled in their native f64 domain"
    )]
    fn scale(self, numerator_scale: u128, denominator_scale: u128) -> Option<Self> {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => Some(Self::Exact {
                numerator: numerator.push(numerator_scale)?,
                denominator: denominator.push(denominator_scale)?,
            }),
            Self::Float(value) => {
                let scaled = value * numerator_scale as f64 / denominator_scale as f64;
                scaled.is_finite().then_some(Self::Float(scaled))
            }
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics are compared to the closest f64 threshold"
    )]
    fn matches(&self, operator: SearchOperator, quantity: &Quantity) -> bool {
        let ordering = match self {
            Self::Exact {
                numerator,
                denominator,
            } => {
                let (Some(left), Some(right)) = (
                    numerator.push(quantity.denominator),
                    denominator.push(quantity.numerator),
                ) else {
                    return false;
                };
                compare_products(left.as_slice(), right.as_slice())
            }
            Self::Float(value) => {
                let threshold = quantity.numerator as f64 / quantity.denominator as f64;
                if !value.is_finite() || !threshold.is_finite() {
                    return false;
                }
                value.total_cmp(&threshold)
            }
        };
        match operator {
            SearchOperator::Greater => ordering == Ordering::Greater,
            SearchOperator::Less => ordering == Ordering::Less,
            SearchOperator::Colon => false,
        }
    }
}

fn result_search_matches(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    clause: &SearchClause,
) -> bool {
    let SearchValue::Quantity(quantity) = &clause.value else {
        return false;
    };
    let Some(field) = result_field(context.logical_name, clause.key) else {
        return false;
    };
    search_metric(context, row, identity, field.metric)
        .is_some_and(|value| value.matches(clause.operator, quantity))
}

fn search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match context.logical_name {
        "os_process" => process_search_metric(context, row, identity, metric),
        "pg_stat_statements" | "pg_store_plans" => {
            postgres_search_metric(context, row, identity, metric)
        }
        "pg_stat_activity" => activity_search_metric(row, metric),
        "pg_stat_progress_vacuum" => vacuum_search_metric(row, metric),
        "pg_stat_database" => database_search_metric(context, row, identity, metric),
        _ => None,
    }
}

/// Compares recorded xid/xmin-age gauges directly; no predecessor or rate is
/// used.
fn activity_search_metric(row: &Row, metric: &str) -> Option<SearchMetricValue> {
    match metric {
        "backend_xid_age" => gauge_metric(row, "backend_xid_age", 1, 1),
        "backend_xmin_age" => gauge_metric(row, "backend_xmin_age", 1, 1),
        _ => None,
    }
}

/// Compares recorded heap-block gauges directly; no predecessor or rate is
/// used.
fn vacuum_search_metric(row: &Row, metric: &str) -> Option<SearchMetricValue> {
    match metric {
        "heap_blks_total" => gauge_metric(row, "heap_blks_total", 1, 1),
        "heap_blks_scanned" => gauge_metric(row, "heap_blks_scanned", 1, 1),
        "heap_blks_vacuumed" => gauge_metric(row, "heap_blks_vacuumed", 1, 1),
        _ => None,
    }
}

/// Compares `numbackends` as a recorded gauge and the four cumulative fields
/// as interval rates.
fn database_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match metric {
        "numbackends" => gauge_metric(row, "numbackends", 1, 1),
        "xact_commit" => rate_metric(context, row, identity, &["xact_commit"], 1, 1),
        "xact_rollback" => rate_metric(context, row, identity, &["xact_rollback"], 1, 1),
        "deadlocks" => rate_metric(context, row, identity, &["deadlocks"], 1, 1),
        "temp_bytes" => rate_metric(context, row, identity, &["temp_bytes"], 1, 1),
        _ => None,
    }
}

fn process_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match metric {
        "rss" => gauge_metric(row, "rmem_kb", 1_024, 1),
        "vsz" => gauge_metric(row, "vmem_kb", 1_024, 1),
        "swap" => gauge_metric(row, "vswap_kb", 1_024, 1),
        "threads" => gauge_metric(row, "num_threads", 1, 1),
        "cpu_cores" | "user_cpu_cores" | "system_cpu_cores" => {
            let columns: &[&str] = match metric {
                "user_cpu_cores" => &["utime"],
                "system_cpu_cores" => &["stime"],
                _ => &["utime", "stime"],
            };
            let ticks = context.clock_ticks_per_second?;
            rate_metric(context, row, identity, columns, 1_000_000, ticks)
        }
        "disk_read_rate" => rate_metric(context, row, identity, &["read_bytes"], 1_000_000, 1),
        "disk_write_rate" => rate_metric(context, row, identity, &["write_bytes"], 1_000_000, 1),
        "logical_read_rate" => rate_metric(context, row, identity, &["rchar"], 1_000_000, 1),
        "logical_write_rate" => rate_metric(context, row, identity, &["wchar"], 1_000_000, 1),
        "read_syscall_rate" => rate_metric(context, row, identity, &["syscr"], 1_000_000, 1),
        "write_syscall_rate" => rate_metric(context, row, identity, &["syscw"], 1_000_000, 1),
        "major_fault_rate" => rate_metric(context, row, identity, &["majflt"], 1_000_000, 1),
        "minor_fault_rate" => rate_metric(context, row, identity, &["minflt"], 1_000_000, 1),
        "context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw", "nivcsw"], 1_000_000, 1)
        }
        "voluntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw"], 1_000_000, 1)
        }
        "involuntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nivcsw"], 1_000_000, 1)
        }
        "run_delay" => rate_metric(context, row, identity, &["rundelay_ns"], 1, 1),
        "block_io_delay" => rate_metric(
            context,
            row,
            identity,
            &["blkdelay_ticks"],
            1_000_000_000,
            context.clock_ticks_per_second?,
        ),
        _ => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixed public PostgreSQL metric adapter stays exhaustive and fork-transparent"
)]
fn postgres_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    let execution = preferred_column(context.plan, "total_exec_time", "total_time");
    let mean = preferred_column(context.plan, "mean_exec_time", "mean_time");
    let stddev = preferred_column(context.plan, "stddev_exec_time", "stddev_time");
    if metric == "calls" {
        return gauge_metric(row, "calls", 1, 1);
    }
    let direct_rate = match metric {
        "call_rate" => Some("calls"),
        "exec_time_rate" => execution,
        "row_rate" => Some("rows"),
        "plan_rate" => Some("plans"),
        "planning_time_rate" => Some("total_plan_time"),
        "slow_call_rate" => Some("slow_log_calls"),
        "shared_read_time_rate" => {
            preferred_column(context.plan, "shared_blk_read_time", "blk_read_time")
        }
        "shared_write_time_rate" => {
            preferred_column(context.plan, "shared_blk_write_time", "blk_write_time")
        }
        "local_read_time_rate" => Some("local_blk_read_time"),
        "local_write_time_rate" => Some("local_blk_write_time"),
        "temp_read_time_rate" => Some("temp_blk_read_time"),
        "temp_write_time_rate" => Some("temp_blk_write_time"),
        "wal_rate" => Some("wal_bytes"),
        _ => None,
    };
    if let Some(column) = direct_rate {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1);
    }
    let buffer_column = match metric {
        "shared_buffer_hit_rate" => Some("shared_blks_hit"),
        "shared_buffer_read_rate" => Some("shared_blks_read"),
        "shared_buffer_dirty_rate" => Some("shared_blks_dirtied"),
        "shared_buffer_write_rate" => Some("shared_blks_written"),
        "local_buffer_hit_rate" => Some("local_blks_hit"),
        "local_buffer_read_rate" => Some("local_blks_read"),
        "local_buffer_dirty_rate" => Some("local_blks_dirtied"),
        "local_buffer_write_rate" => Some("local_blks_written"),
        "temp_buffer_read_rate" => Some("temp_blks_read"),
        "temp_buffer_write_rate" => Some("temp_blks_written"),
        _ => None,
    };
    if let Some(column) = buffer_column {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1)
            .and_then(|value| value.scale(context.block_size?, 1));
    }
    match metric {
        "mean_exec" => counter_ratio_metric(context, row, identity, &[execution?], &["calls"], 1),
        "rows_per_call" => counter_ratio_metric(context, row, identity, &["rows"], &["calls"], 1),
        "wal_per_call" => {
            counter_ratio_metric(context, row, identity, &["wal_bytes"], &["calls"], 1)
        }
        "planning_share" => counter_ratio_metric(
            context,
            row,
            identity,
            &["total_plan_time"],
            &["total_plan_time", execution?],
            100,
        ),
        "buffer_hit" => counter_ratio_metric(
            context,
            row,
            identity,
            &["shared_blks_hit"],
            &["shared_blks_hit", "shared_blks_read"],
            100,
        ),
        "buffer_per_call" => counter_ratio_metric(
            context,
            row,
            identity,
            &[
                "shared_blks_hit",
                "shared_blks_read",
                "shared_blks_dirtied",
                "shared_blks_written",
                "local_blks_hit",
                "local_blks_read",
                "local_blks_dirtied",
                "local_blks_written",
                "temp_blks_read",
                "temp_blks_written",
            ],
            &["calls"],
            context.block_size?,
        ),
        "exec_cv" => value_ratio_metric(row, &[stddev?], &[mean?], 1),
        "min_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "min_exec_time", "min_time")?,
            1,
            1,
        ),
        "max_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "max_exec_time", "max_time")?,
            1,
            1,
        ),
        "mean_exec_since_reset" => gauge_metric(row, mean?, 1, 1),
        "stddev_exec_since_reset" => gauge_metric(row, stddev?, 1, 1),
        _ => None,
    }
}

fn preferred_column(
    plan: &Plan,
    preferred: &'static str,
    fallback: &'static str,
) -> Option<&'static str> {
    plan.contract
        .column(preferred)
        .map(|column| column.name)
        .or_else(|| plan.contract.column(fallback).map(|column| column.name))
}

fn gauge_metric(
    row: &Row,
    column: &'static str,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    ordered_metric(
        ordered_cell(row.get(column)?)?,
        numerator_scale,
        denominator_scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn rate_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    columns: &[&'static str],
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    let elapsed = u128::try_from(context.elapsed_for(row)?)
        .ok()
        .filter(|value| *value > 0)?;
    let before = context.predecessor(row, identity)?;
    let delta = counter_sum(row, before, columns, false)?;
    match delta {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::two(elapsed, denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted =
                value * numerator_scale as f64 / elapsed as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

fn counter_ratio_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    let _elapsed = context.elapsed_for(row)?;
    let before = context.predecessor(row, identity)?;
    let numerator = counter_sum(row, before, numerator_columns, false)?;
    let denominator = counter_sum(row, before, denominator_columns, false)?;
    ratio_metric(numerator, denominator, scale)
}

fn value_ratio_metric(
    row: &Row,
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    ratio_metric(
        value_sum(row, numerator_columns)?,
        value_sum(row, denominator_columns)?,
        scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn ratio_metric(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
    scale: u128,
) -> Option<SearchMetricValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(SearchMetricValue::Exact {
                numerator: ProductFactors::two(u128::try_from(numerator).ok()?, scale),
                denominator: ProductFactors::one(u128::try_from(denominator).ok()?),
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator * scale as f64;
            (denominator > 0.0 && ratio.is_finite()).then_some(SearchMetricValue::Float(ratio))
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating gauges preserve their recorded f64 value"
)]
fn ordered_metric(
    value: OrderedNumber,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    match value {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::one(denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted = value * numerator_scale as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

fn search_clause_columns(logical_name: &str, plan: &Plan, key: &str) -> Vec<&'static str> {
    let Some(field) = search_fields(logical_name)
        .iter()
        .find(|field| field.key == key)
    else {
        return Vec::new();
    };
    let wanted: &[&str] = if logical_name == "pg_store_plans" && key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        field.columns
    };
    wanted
        .iter()
        .filter_map(|name| plan.contract.column(name).map(|column| column.name))
        .collect()
}

fn search_clause_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    clause: &SearchClause,
) -> bool {
    let excludes_zero_query =
        logical_name == "pg_store_plans" && plan.type_id == 1_004_001 && clause.key == "query_id";
    if logical_name == "os_process" {
        let name_matches = |uid_column| {
            process_users
                .and_then(|users| users.for_row(row, uid_column))
                .is_some_and(|name| search_value_matches(name, &clause.value))
        };
        match clause.key {
            "user" => return name_matches("uid"),
            "effective_user" => return name_matches("euid"),
            "text" => {
                if name_matches("uid") || name_matches("euid") {
                    return true;
                }
                return ["comm", "cmdline"].iter().any(|column| {
                    row.get(column)
                        .and_then(|value| searchable_text(value, dictionary))
                        .is_some_and(|text| search_value_matches(&text, &clause.value))
                });
            }
            _ => {}
        }
    }
    let columns = if logical_name == "pg_store_plans" && clause.key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        clause.columns
    };
    columns.iter().any(|column| {
        row.get(column)
            .and_then(|value| searchable_text(value, dictionary))
            .is_some_and(|text| {
                (!excludes_zero_query || text != "0") && search_value_matches(&text, &clause.value)
            })
    })
}

fn search_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    search: &StructuredSearch,
) -> bool {
    search.matches_member(|clause| {
        search_clause_matches(logical_name, plan, row, dictionary, process_users, clause)
    })
}

fn searchable_text<'a>(value: &Cell, dictionary: &'a Dictionary) -> Option<Cow<'a, str>> {
    match value {
        Cell::I16(value) => Some(Cow::Owned(value.to_string())),
        Cell::I32(value) => Some(Cow::Owned(value.to_string())),
        Cell::I64(value) | Cell::Ts(value) => Some(Cow::Owned(value.to_string())),
        Cell::U32(value) => Some(Cow::Owned(value.to_string())),
        Cell::U64(value) => Some(Cow::Owned(value.to_string())),
        Cell::F64(value) if value.is_finite() => Some(Cow::Owned(value.to_string())),
        Cell::Bool(value) => Some(Cow::Owned(value.to_string())),
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .and_then(|resolved| std::str::from_utf8(resolved.stored_bytes()).ok())
            .map(Cow::Borrowed),
        Cell::Null | Cell::ListI32(_) | Cell::F64(_) => None,
    }
}

impl SnapshotCursor {
    fn parse(raw: &str) -> Result<Self, QueryError> {
        let fields = raw.split(',').collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(QueryError::BadCursor);
        }
        Ok(Self {
            segment_id: fields[0].parse().map_err(|_error| QueryError::BadCursor)?,
            active_position: fields[1].parse().map_err(|_error| QueryError::BadCursor)?,
            context_index: fields[2].parse().map_err(|_error| QueryError::BadCursor)?,
            ordinal: fields[3].parse().map_err(|_error| QueryError::BadCursor)?,
            binding: fields[4].parse().map_err(|_error| QueryError::BadCursor)?,
        })
    }

    fn encode(self) -> String {
        format!(
            "{},{},{},{},{}",
            self.segment_id, self.active_position, self.context_index, self.ordinal, self.binding
        )
    }
}

const fn partition_context_index(
    layout_index: usize,
    source: PartitionSource,
    predecessor_count: usize,
) -> usize {
    layout_index * (predecessor_count + 1)
        + match source {
            PartitionSource::Current => 0,
            PartitionSource::Earlier(index) => predecessor_count - index,
        }
}

const fn timed_context_index(
    layout_index: usize,
    source_index: usize,
    source_count: usize,
) -> usize {
    layout_index * source_count + source_index
}

fn pin(
    dataset: &dyn QueryDataset,
    current: DatasetSegment,
    cursor: Option<SnapshotCursor>,
) -> Result<DatasetSegment, QueryError> {
    let Some(cursor) = cursor else {
        return Ok(current);
    };
    match current.kind() {
        SegmentKind::Finished if cursor.active_position == 0 => Ok(current),
        SegmentKind::Active => dataset
            .at_active_position(&current, cursor.active_position)
            .map_err(|_error| QueryError::BadCursor),
        SegmentKind::Finished => Err(QueryError::BadCursor),
    }
}

fn snapshot_binding(request: &SnapshotRequest, search: Option<&StructuredSearch>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_part(&mut hash, b"segment", &request.segment_id.to_le_bytes());
    hash_part(&mut hash, b"at", &request.at.to_le_bytes());
    for section in &request.sections {
        hash_part(&mut hash, b"section", section.as_bytes());
    }
    if let Some(type_id) = request.type_id {
        hash_part(&mut hash, b"type", &type_id.to_le_bytes());
    }
    for field in &request.fields {
        hash_part(&mut hash, b"field", field.as_bytes());
    }
    if let Some(text) = request.text {
        hash_part(&mut hash, b"text", &text.to_le_bytes());
    }
    for filter in &request.filters {
        hash_part(&mut hash, b"filter-column", filter.column.as_bytes());
        hash_part(&mut hash, b"filter-value", filter.value.as_bytes());
    }
    if let Some(search) = search {
        hash_part(&mut hash, b"search", search.canonical().as_bytes());
    }
    hash_part(&mut hash, b"first-match", &[u8::from(request.first_match)]);
    hash_part(&mut hash, b"scope", request.scope.as_str().as_bytes());
    for by in &request.by {
        hash_part(&mut hash, b"by", by.as_bytes());
    }
    if let Some(group) = request.group {
        hash_part(&mut hash, b"group", group.as_str().as_bytes());
    }
    hash_part(
        &mut hash,
        b"direction",
        request.direction.as_str().as_bytes(),
    );
    hash
}

fn hash_part(hash: &mut u64, tag: &[u8], bytes: &[u8]) {
    hash_bytes(hash, tag);
    hash_bytes(hash, bytes);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    for byte in len.to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn retained_dictionary(segment: &Segment, rows: &[StagedRow]) -> Result<Dictionary, QueryError> {
    let ids: HashSet<u64> = rows
        .iter()
        .flat_map(|staged| staged.row.iter())
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect();
    resolved_dictionary(segment, &ids)
}

#[cfg(test)]
fn available_field_index(fields: &[crate::OutputField], name: &str) -> Option<usize> {
    fields
        .iter()
        .position(|field| field.name == name && field.column.is_some())
}

struct RetainedMoment {
    at: i64,
    segment_ids: BTreeSet<i64>,
}

#[derive(Default)]
struct RetainedMoments {
    current: Option<RetainedMoment>,
    previous: Option<RetainedMoment>,
}

type RetainedRelationMoments = BTreeMap<(u32, IdentityCell), RetainedMoments>;

fn located_moment(
    retained: &RetainedMoment,
    source_by_id: &HashMap<i64, PartitionSource>,
) -> Option<LocatedMoment> {
    let sources = retained
        .segment_ids
        .iter()
        .filter_map(|segment_id| source_by_id.get(segment_id).copied())
        .collect::<BTreeSet<_>>();
    (!sources.is_empty()).then_some(LocatedMoment {
        at: retained.at,
        sources,
    })
}

fn relation_preceding(
    dataset: &dyn QueryDataset,
    segment_ref: &DatasetSegment,
    segments: Vec<DatasetSegment>,
    current: &Segment,
    sections: &[SectionPlans],
    filters: &[crate::Filter],
    at: i64,
) -> Result<(Vec<DatasetSegment>, RetainedRelationMoments), QueryError> {
    let requested_datid = filters
        .iter()
        .find(|filter| filter.column == "datid")
        .and_then(|filter| filter.value.parse::<u32>().ok())
        .map(IdentityCell::U32);
    let mut moments = BTreeMap::<(u32, IdentityCell), RetainedMoments>::new();
    if let Some(datid) = requested_datid.as_ref() {
        for plan in partitioned_plans(sections) {
            if plan.applies() && plan.timestamp.is_some() {
                moments.entry((plan.type_id, datid.clone())).or_default();
            }
        }
    }
    scan_relation_moments(
        current,
        sections,
        requested_datid.as_ref(),
        requested_datid.is_none(),
        at,
        &mut moments,
    )?;
    let compatible = partitioned_plans(sections)
        .map(|plan| plan.type_id)
        .collect::<HashSet<_>>();
    let mut candidates = segments
        .into_iter()
        .filter(|candidate| candidate.id() < segment_ref.id() && candidate.min_ts() <= at)
        .filter(|candidate| {
            candidate
                .sections()
                .iter()
                .any(|section| compatible.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if !moments.is_empty()
            && moments.values().all(|samples| {
                samples
                    .previous
                    .as_ref()
                    .is_some_and(|previous| candidate.max_ts() < previous.at)
            })
        {
            break;
        }
        let segment = dataset.open(candidate)?;
        scan_relation_moments(
            &segment,
            sections,
            requested_datid.as_ref(),
            requested_datid.is_none(),
            at,
            &mut moments,
        )?;
    }

    let mut retained = HashSet::new();
    for samples in moments.values() {
        for sample in [&samples.current, &samples.previous].into_iter().flatten() {
            for segment_id in &sample.segment_ids {
                if *segment_id != segment_ref.id() {
                    retained.insert(*segment_id);
                }
            }
        }
    }
    let mut selected = candidates
        .into_iter()
        .filter(|candidate| retained.contains(&candidate.id()))
        .collect::<Vec<_>>();
    selected.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok((selected, moments))
}

fn partitioned_plans(sections: &[SectionPlans]) -> impl Iterator<Item = &Plan> {
    sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some())
        .flat_map(|section| section.plans.iter())
}

fn scan_relation_moments(
    segment: &Segment,
    sections: &[SectionPlans],
    requested_datid: Option<&IdentityCell>,
    discover: bool,
    at: i64,
    moments: &mut BTreeMap<(u32, IdentityCell), RetainedMoments>,
) -> Result<(), QueryError> {
    for plan in partitioned_plans(sections) {
        if !plan.applies() {
            continue;
        }
        let Some(timestamp) = plan.timestamp else {
            continue;
        };
        if segment.rows_of(plan.type_id).is_none() {
            continue;
        }
        #[cfg(test)]
        RELATION_MOMENT_VISITS.set(RELATION_MOMENT_VISITS.get().saturating_add(1));
        segment.visit_rows(
            plan.type_id,
            &[timestamp, "datid"],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(stored), Some(partition)) =
                    (row_timestamp(&row, timestamp), row.get("datid"))
                else {
                    return true;
                };
                let partition = identity_cell(partition);
                if requested_datid.is_some_and(|requested| requested != &partition) {
                    return true;
                }
                let key = (plan.type_id, partition);
                if discover {
                    moments.entry(key.clone()).or_default();
                }
                if stored <= at
                    && let Some(selected) = moments.get_mut(&key)
                {
                    record_retained_moment(
                        selected,
                        RetainedMoment {
                            at: stored,
                            segment_ids: BTreeSet::from([segment.id()]),
                        },
                    );
                }
                true
            },
        )?;
    }
    Ok(())
}

fn record_retained_moment(moments: &mut RetainedMoments, sample: RetainedMoment) {
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(sample);
        return;
    };
    match sample.at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.extend(sample.segment_ids);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.replace(sample);
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match sample.at.cmp(&previous.at) {
                Ordering::Equal => previous.segment_ids.extend(sample.segment_ids),
                Ordering::Greater => *previous = sample,
                Ordering::Less => {}
            },
            None => moments.previous = Some(sample),
        },
    }
}

struct ContributingMoment {
    at: i64,
    segment_ids: HashSet<i64>,
}

#[derive(Default)]
struct ContributingMoments {
    current: Option<ContributingMoment>,
    previous: Option<ContributingMoment>,
    pinned: bool,
}

fn preceding(
    dataset: &dyn QueryDataset,
    segment_ref: &DatasetSegment,
    segments: Vec<DatasetSegment>,
    current: &Segment,
    sections: &[SectionPlans],
    at: i64,
    pin_current: bool,
) -> Result<Vec<DatasetSegment>, QueryError> {
    let layouts = sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_none())
        .flat_map(|section| section.plans.iter())
        .filter(|plan| plan.applies())
        .filter_map(|plan| plan.timestamp.map(|timestamp| (plan.type_id, timestamp)))
        .collect::<BTreeMap<_, _>>();
    if layouts.is_empty() {
        return Ok(Vec::new());
    }
    let compatible = layouts.keys().copied().collect::<HashSet<_>>();
    let mut candidates = segments
        .into_iter()
        .filter(|candidate| candidate.id() < segment_ref.id() && candidate.min_ts() <= at)
        .filter(|candidate| {
            candidate
                .sections()
                .iter()
                .any(|section| compatible.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    let mut moments = layouts
        .keys()
        .map(|type_id| (*type_id, ContributingMoments::default()))
        .collect::<BTreeMap<_, _>>();
    scan_contributing_moments(current, &layouts, at, pin_current, &mut moments)?;
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if moments.values().all(|samples| {
            samples
                .previous
                .as_ref()
                .is_some_and(|previous| candidate.max_ts() < previous.at)
        }) {
            break;
        }
        let segment = dataset.open(candidate)?;
        scan_contributing_moments(&segment, &layouts, at, false, &mut moments)?;
    }
    let retained = moments
        .values()
        .flat_map(|moments| [&moments.current, &moments.previous])
        .filter_map(Option::as_ref)
        .flat_map(|moment| moment.segment_ids.iter().copied())
        .filter(|segment_id| *segment_id != segment_ref.id())
        .collect::<HashSet<_>>();
    candidates.retain(|candidate| retained.contains(&candidate.id()));
    candidates.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok(candidates)
}

fn scan_contributing_moments(
    segment: &Segment,
    layouts: &BTreeMap<u32, &'static str>,
    at: i64,
    pin_current: bool,
    moments: &mut BTreeMap<u32, ContributingMoments>,
) -> Result<(), QueryError> {
    for (&type_id, &timestamp) in layouts {
        if segment.rows_of(type_id).is_none() {
            continue;
        }
        segment.visit_rows(type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
            if let Some(stored) = row_timestamp(&row, timestamp)
                && stored <= at
            {
                record_contributing_moment(
                    moments.entry(type_id).or_default(),
                    stored,
                    segment.id(),
                );
            }
            true
        })?;
    }
    if pin_current {
        for samples in moments.values_mut() {
            samples.pinned = samples.current.is_some();
        }
    }
    Ok(())
}

fn record_contributing_moment(moments: &mut ContributingMoments, at: i64, segment_id: i64) {
    let new_moment = || ContributingMoment {
        at,
        segment_ids: HashSet::from([segment_id]),
    };
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(new_moment());
        return;
    };
    if moments.pinned && at > current_at {
        return;
    }
    match at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.insert(segment_id);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.take();
            moments.current = Some(new_moment());
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match at.cmp(&previous.at) {
                Ordering::Equal => {
                    previous.segment_ids.insert(segment_id);
                }
                Ordering::Greater => moments.previous = Some(new_moment()),
                Ordering::Less => {}
            },
            None => moments.previous = Some(new_moment()),
        },
    }
}

fn validate_shared_projection(
    segment: &Segment,
    sections: &[String],
    fields: &[String],
) -> Result<(), QueryError> {
    for field in fields {
        let known = sections.iter().any(|section| {
            segment
                .layouts(section)
                .filter_map(|(type_id, _section)| contract(type_id))
                .any(|layout| layout.column(field).is_some())
        });
        if !known {
            return Err(QueryError::NoSuchColumn(field.clone()));
        }
    }
    Ok(())
}

fn section_projection(segment: &Segment, logical_name: &str, fields: &[String]) -> Vec<String> {
    let columns = segment
        .layouts(logical_name)
        .filter_map(|(type_id, _section)| contract(type_id))
        .flat_map(|layout| layout.columns.iter().map(|column| column.name))
        .collect::<HashSet<_>>();
    fields
        .iter()
        .filter(|field| columns.contains(field.as_str()))
        .cloned()
        .collect()
}

struct Moments {
    current: i64,
    previous: Option<i64>,
}

/// Lifetime CPU time of one process, in clock ticks.
///
/// Cumulative columns leave a snapshot row as rates, so the total the process
/// has burned since it started is served separately.
fn scheduled_ticks(row: &Row) -> Value {
    let ticks = |column| match row.get(column) {
        Some(&Cell::I64(value)) => Some(value),
        _ => None,
    };
    match (ticks("utime"), ticks("stime")) {
        (Some(user), Some(system)) => user
            .checked_add(system)
            .map_or(Value::Null, |total| Value::String(total.to_string())),
        _ => Value::Null,
    }
}

/// Returns null without a valid nondecreasing predecessor.
fn rate(
    stored: Option<&Cell>,
    before: Option<&CounterReadings>,
    column: &'static str,
    elapsed: Option<i64>,
) -> Value {
    let (Some(now), Some(before), Some(elapsed)) = (stored, before, elapsed) else {
        return Value::Null;
    };
    let Some(earlier) = before.get(column) else {
        return Value::Null;
    };
    let Some(delta) = counter_delta(now, earlier) else {
        return Value::Null;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    let seconds = elapsed as f64 / 1_000_000.0;
    let value = delta.as_f64() / seconds;
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

#[derive(Clone, Copy)]
pub(crate) enum OrderedNumber {
    Integer(i128),
    Float(f64),
}

impl OrderedNumber {
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer counter deltas are converted only after exact subtraction"
    )]
    const fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

fn counter_delta(now: &Cell, earlier: &Cell) -> Option<OrderedNumber> {
    let exact = match (now, earlier) {
        (Cell::I16(now), Cell::I16(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I32(now), Cell::I32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I64(now) | Cell::Ts(now), Cell::I64(earlier) | Cell::Ts(earlier)) => {
            i128::from(*now) - i128::from(*earlier)
        }
        (Cell::U32(now), Cell::U32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::U64(now), Cell::U64(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::F64(now), Cell::F64(earlier)) => {
            let delta = now - earlier;
            return (delta >= 0.0 && delta.is_finite()).then_some(OrderedNumber::Float(delta));
        }
        _ => return None,
    };
    (exact >= 0).then_some(OrderedNumber::Integer(exact))
}

fn output_rate_fields(plan: &Plan) -> Vec<&str> {
    plan.fields
        .iter()
        .filter_map(|field| {
            let column = field.column?;
            let cumulative = plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
            let exact_plan_calls =
                matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001) && field.name == "calls";
            (cumulative && !exact_plan_calls).then_some(field.name.as_str())
        })
        .collect()
}

fn projected_rate_columns(plan: &Plan) -> Vec<&'static str> {
    plan.projection
        .iter()
        .copied()
        .filter(|column| {
            plan.contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
        })
        .collect()
}

fn identity_of(plan: &Plan, row: &Row) -> Option<Vec<IdentityCell>> {
    if plan.contract.identity.is_empty() {
        return Some(Vec::new());
    }
    let mut identity = Vec::with_capacity(plan.contract.identity.len());
    for name in plan.contract.identity {
        let stored = row.get(name)?;
        identity.push(identity_cell(stored));
    }
    Some(identity)
}

fn identity_cell(stored: &Cell) -> IdentityCell {
    match stored {
        Cell::Null => IdentityCell::Null,
        Cell::I16(value) => IdentityCell::I16(*value),
        Cell::I32(value) => IdentityCell::I32(*value),
        Cell::I64(value) => IdentityCell::I64(*value),
        Cell::Ts(value) => IdentityCell::Ts(*value),
        Cell::U32(value) => IdentityCell::U32(*value),
        Cell::U64(value) => IdentityCell::U64(*value),
        Cell::F64(value) => IdentityCell::F64(value.to_bits()),
        Cell::Bool(value) => IdentityCell::Bool(*value),
        Cell::ListI32(value) => IdentityCell::ListI32(value.clone()),
        Cell::StrId(id) => IdentityCell::StrId(*id),
    }
}

fn row_timestamp(row: &Row, column: &'static str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _other => None,
    }
}

fn encoded_locator_identity(plan: &Plan, row: &Row) -> Result<(i64, String), QueryError> {
    let timestamp = plan.timestamp.ok_or(QueryError::BadCursor)?;
    let at = row_timestamp(row, timestamp).ok_or_else(|| {
        QueryError::BadLocator(format!(
            "cannot emit detail_locator: type_id {} row has no timestamp",
            plan.type_id,
        ))
    })?;
    let identity = crate::identity(plan.type_id, row).map_err(QueryError::BadLocator)?;
    Ok((at, serde_json::to_string(&identity)?))
}

fn non_unique_locator(context: &PageContext<'_>, at: i64) -> QueryError {
    QueryError::BadLocator(format!(
        "cannot emit detail_locator: {} has a non-unique identity at timestamp {at} in segment {}",
        context.logical_name,
        context.source.id(),
    ))
}

#[cfg(test)]
mod tests;
