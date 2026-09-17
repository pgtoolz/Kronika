//! Grouped relation histories for `PostgreSQL` tables and indexes.

mod aggregate;
mod fields;
mod history;
mod identity;
mod metric;

pub use fields::{key_fields, output_fields};
pub(super) use history::stream_history;
pub use metric::index_scan_rate_is_zero;

use std::collections::BTreeMap;

use kronika_reader::{Cell, Dictionary, Resolved, Row};
use kronika_registry::ColumnClass;

use crate::projection::Plan;
use crate::{DatasetSegment, QueryError};

const HISTORY_CHUNK_ROWS: usize = 1_024;

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

#[derive(Clone, Copy)]
enum OrderedNumber {
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

fn rate_columns(plan: &Plan) -> Vec<&'static str> {
    plan.fields
        .iter()
        .filter_map(|field| field.column)
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
        identity.push(identity_cell(row.get(name)?));
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

fn counter_input(
    plan: &Plan,
    row: &Row,
    before: Option<&CounterReadings>,
    name: &'static str,
    structural: bool,
) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    let (Some(now), Some(earlier)) = (row.get(name), before.and_then(|values| values.get(name)))
    else {
        return Input::Unavailable;
    };
    if structural && matches!((now, earlier), (Cell::Null, Cell::Null)) {
        return Input::Neutral;
    }
    counter_delta(now, earlier).map_or(Input::Unavailable, Input::Value)
}

fn gauge_input(plan: &Plan, row: &Row, name: &'static str, structural: bool) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    match row.get(name) {
        Some(Cell::Null) if structural => Input::Neutral,
        Some(cell) => ordered_cell(cell).map_or(Input::Unavailable, Input::Value),
        None => Input::Unavailable,
    }
}

fn text_cell(stored: Option<&Cell>, dictionary: &Dictionary) -> Result<Option<String>, QueryError> {
    let Some(Cell::StrId(id)) = stored else {
        return Ok(None);
    };
    let bytes = dictionary
        .resolve(*id)
        .map(Resolved::stored_bytes)
        .ok_or(QueryError::BadCursor)?;
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|error| QueryError::Unreadable(Box::new(error)))
}

const fn unsigned_cell(stored: Option<&Cell>) -> Option<u32> {
    match stored {
        Some(Cell::U32(value)) => Some(*value),
        _ => None,
    }
}

fn integer_cell(stored: Option<&Cell>) -> Option<i128> {
    ordered_cell(stored?).and_then(|value| match value {
        OrderedNumber::Integer(value) => Some(value),
        OrderedNumber::Float(_) => None,
    })
}

const fn timestamp_cell(stored: Option<&Cell>) -> Option<i64> {
    match stored {
        Some(Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

const fn bool_cell(stored: Option<&Cell>) -> Option<bool> {
    match stored {
        Some(Cell::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn number_is_zero(value: OrderedNumber) -> bool {
    match value {
        OrderedNumber::Integer(value) => value == 0,
        OrderedNumber::Float(value) => value == 0.0,
    }
}

/// Supported relation query families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// User-table statistics.
    Tables,
    /// User-index statistics.
    Indexes,
}

/// One field in the stable relation result contract.
#[derive(Debug, Clone, Copy)]
pub struct RelationField {
    name: &'static str,
    kind: Kind,
    unit: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Number,
    Id,
    Integer,
    Timestamp,
    Boolean,
    Text,
}

/// Opaque stable identity of one relation group.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupKey(GroupKeyValue);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeyValue {
    Database {
        datid: u32,
        datname: String,
    },
    Schema {
        datid: u32,
        datname: String,
        schemaname: String,
    },
    Tablespace {
        tablespace_oid: u32,
    },
    Table {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
    },
    Index {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
        indexrelid: u32,
        indexrelname: String,
    },
}

/// Stable physical source coordinates retained by the relation reducer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelationSource {
    segment_id: i64,
    context_index: usize,
    ordinal: u64,
    type_id: u32,
    timestamp: i64,
}

#[derive(Clone, Copy, Default)]
enum Availability {
    #[default]
    Empty,
    Value(OrderedNumber),
    Unavailable,
}

#[derive(Clone, Copy)]
enum Input {
    Value(OrderedNumber),
    Neutral,
    Unavailable,
}

#[expect(
    variant_size_differences,
    reason = "exact rational rates avoid heap allocation and preserve ordering"
)]
#[derive(Clone, Copy, Debug)]
enum RateValue {
    /// A non-negative rate represented exactly as units per microsecond.
    Exact { numerator: u128, denominator: u128 },
    /// Units per microsecond when the input counter is floating point or exact
    /// rational accumulation exceeds `u128`.
    Float(f64),
}

#[derive(Clone, Copy, Default)]
struct RateAggregate {
    value: Option<RateValue>,
    unavailable: bool,
}

#[derive(Clone, Copy, Default)]
struct MaximumAggregate {
    maximum: Option<i128>,
    unavailable: bool,
}

#[derive(Clone, Copy, Default)]
struct TimestampAggregate {
    applicable: u64,
    unavailable: bool,
    oldest: Option<i64>,
    latest: Option<i64>,
    never: u64,
}

#[derive(Clone, Copy)]
enum TimestampObservation<'a> {
    Unavailable,
    NotApplicable,
    Stored(Option<&'a Cell>),
}

#[derive(Clone, Copy, Default)]
struct BoolAggregate {
    known: u64,
    truthy: u64,
    unavailable: bool,
}

/// Incremental reducer for one relation grouping key.
#[derive(Clone)]
pub struct RelationAggregate {
    key: GroupKey,
    count: u64,
    source: RelationSource,
    from: Option<i64>,
    to: Option<i64>,
    rates: BTreeMap<&'static str, RateAggregate>,
    gauges: BTreeMap<&'static str, Availability>,
    maxima: BTreeMap<&'static str, MaximumAggregate>,
    timestamps: BTreeMap<&'static str, TimestampAggregate>,
    flags: BTreeMap<&'static str, BoolAggregate>,
    texts: BTreeMap<&'static str, String>,
    tablespace_label_timestamp: Option<i64>,
    identifiers: BTreeMap<&'static str, i128>,
    no_scans: BoolAggregate,
    state_severity: Option<i128>,
}

/// Opaque relation metric shared with transport adapters.
#[derive(Clone)]
pub struct Metric {
    value: MetricValue,
}

#[derive(Clone)]
enum MetricValue {
    Number(OrderedNumber),
    Rate(RateValue),
    RateRatio {
        numerator: RateValue,
        denominator: RateValue,
        scale: f64,
    },
    Ratio {
        numerator: OrderedNumber,
        denominator: OrderedNumber,
        scale: f64,
    },
    Integer(i128),
    Timestamp(i64),
    Boolean(bool),
    Text(String),
}

#[derive(Clone, Copy)]
enum MetricOrderValue<'a> {
    Integer(i128),
    Float(f64),
    ExactRatio { numerator: u128, denominator: u128 },
    FloatRatio(f64),
    Text(&'a [u8]),
}

struct HistorySegment {
    segment: DatasetSegment,
    plans: Vec<Plan>,
}

struct HistoryPrevious {
    timestamp: i64,
    readings: CounterReadings,
}

#[derive(Clone, Copy)]
struct HistoryMoment {
    type_id: u32,
    previous: Option<i64>,
}

struct RelationRow {
    key: GroupKey,
    metrics: BTreeMap<String, Option<Metric>>,
    from: Option<i64>,
    to: Option<i64>,
}

#[cfg(test)]
use fields::{INDEXES, TABLES};
#[cfg(test)]
use history::relation_values;
#[cfg(test)]
#[path = "../tests/hour_relation.rs"]
mod tests;
