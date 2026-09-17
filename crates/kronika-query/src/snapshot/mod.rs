//! Reads one snapshot and derives counter rates.

mod cgroup;
mod contexts;
mod cursor;
mod filter;
mod output;
mod paging;
mod predecessor;
mod preparation;
mod relation;
mod search;
mod selector;
mod stream;

pub use preparation::{SnapshotPreparation, prepare_snapshot};
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

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
use std::sync::Arc;

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::ColumnClass;
use serde_json::{Value, json};

use crate::StatementScope;
use crate::dataset::{DatasetSegment, QueryDataset};
use crate::projection::{Plan, resolved_dictionary};
use crate::{Order, QueryError, QuerySink, QueryStability, RelationGroup};

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
    CounterDelta(&'static str),
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

type RetainedRelationMoments = BTreeMap<(u32, IdentityCell), RetainedMoments>;

type RankedRecord<'a> = (&'a Plan, Value, crate::RowIdentity);
type RankedLocatorKey = (usize, i64, String);

#[derive(Default)]
struct ContributingMoments {
    current: Option<ContributingMoment>,
    previous: Option<ContributingMoment>,
    pinned: bool,
}

struct ContributingMoment {
    at: i64,
    segment_ids: HashSet<i64>,
}

#[derive(Default)]
struct RetainedMoments {
    current: Option<RetainedMoment>,
    previous: Option<RetainedMoment>,
}

struct RetainedMoment {
    at: i64,
    segment_ids: BTreeSet<i64>,
}

/// Locator and positional values extracted from `row_record` output.
struct RowLocator {
    segment_id: i64,
    row_ordinal: u64,
    at: i64,
    values: Vec<Value>,
}

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
use paging::compare_ordered;
#[cfg(test)]
use paging::{compare_page_order_values, compare_u128_ratios};
#[cfg(test)]
use predecessor::record_contributing_moment;
#[cfg(test)]
use preparation::prepared_search;
#[cfg(test)]
use std::cell::Cell as Counter;
#[cfg(test)]
#[path = "../tests/snapshot.rs"]
mod tests;
