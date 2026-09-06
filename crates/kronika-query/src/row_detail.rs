//! One exact stored row addressed by an opaque reference.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use kronika_reader::{Cell, Resolved, Row, Segment};
use kronika_registry::{ColumnClass, Semantics, TypeContract, contract};
use serde_json::{Map, Value, json};

use super::projection::chunk_dictionary;
use super::render::{cell, record};
use super::row_key::{self, DetailLocator};
use crate::{
    DatasetSegment, QueryContext, QueryDataset, QueryError, QuerySink, SegmentBounds,
    SegmentSelection,
};

const PROCESS_USER_TYPE_ID: u32 = 1_124_002;
const MAX_PROCESS_USERS: usize = 4 * 1024;

/// Validated opaque request for one recorded row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRowDetailQuery {
    locator: DetailLocator,
}

/// Typed row-detail result shared by native protocol adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowDetailResult {
    /// Stable logical section name.
    pub section: String,
    /// Exact recorded timestamp, in Unix microseconds.
    pub at: i64,
    /// Complete rendered row, including its decimal-string `at` member.
    pub fields: Map<String, Value>,
}

/// One prepared row-detail lookup over a captured catalog.
pub struct PreparedRowDetail {
    dataset: Arc<dyn QueryDataset>,
    request: ValidatedRowDetailQuery,
    anchor: DatasetSegment,
    anchor_segment: Segment,
    other_segments: Vec<DatasetSegment>,
}

impl std::fmt::Debug for PreparedRowDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRowDetail")
            .field("request", &self.request)
            .field("anchor", &self.anchor)
            .field("other_segments", &self.other_segments)
            .finish_non_exhaustive()
    }
}

/// Validate an opaque row reference without opening its data source.
///
/// # Errors
///
/// Returns `bad_locator` for every malformed, oversized, or noncanonical
/// reference.
pub fn validate_row_detail_ref(detail_ref: &str) -> Result<ValidatedRowDetailQuery, QueryError> {
    DetailLocator::from_detail_ref(detail_ref)
        .map(|locator| ValidatedRowDetailQuery { locator })
        .map_err(|_error| QueryError::BadLocator("invalid detail_ref".to_owned()))
}

/// Capture and prepare one validated row-detail lookup.
///
/// # Errors
///
/// Returns a catalog or captured-source error when preparation cannot select
/// the referenced segment.
pub fn prepare_row_detail(
    context: &QueryContext,
    request: ValidatedRowDetailQuery,
) -> Result<PreparedRowDetail, QueryError> {
    let catalog = context.dataset.catalog()?;
    let mut listing = catalog.segments(SegmentSelection::new(SegmentBounds::all()))?;
    let Some(index) = listing
        .segments
        .iter()
        .position(|segment| segment.id() == request.locator.segment_id)
    else {
        return Err(QueryError::NoSuchSegment);
    };
    let anchor = listing.segments.remove(index);
    let anchor_segment = context.dataset.open(&anchor)?;
    Ok(PreparedRowDetail {
        dataset: Arc::clone(&context.dataset),
        request,
        anchor,
        anchor_segment,
        other_segments: listing.segments,
    })
}

/// Execute one typed row-detail lookup without serializing an HTTP record.
///
/// # Errors
///
/// Returns a locator, cancellation, decoding, or captured-source error.
pub fn execute_row_detail(
    context: &QueryContext,
    request: ValidatedRowDetailQuery,
    sink: &dyn QuerySink,
) -> Result<RowDetailResult, QueryError> {
    prepare_row_detail(context, request)?.execute(sink)
}

impl PreparedRowDetail {
    /// Resolve and semantically render the referenced row.
    ///
    /// # Errors
    ///
    /// Returns a locator, cancellation, decoding, or captured-source error.
    pub fn execute(&self, sink: &dyn QuerySink) -> Result<RowDetailResult, QueryError> {
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        let locator = &self.request.locator;
        let segment = &self.anchor_segment;
        let Some(rows) = segment.rows_of(locator.type_id) else {
            return Err(QueryError::BadCursor);
        };
        let Some(contract) = contract(locator.type_id).filter(|candidate| {
            segment
                .layouts(&locator.section)
                .any(|(type_id, _section)| type_id == candidate.type_id.get())
        }) else {
            return Err(QueryError::BadCursor);
        };
        let Some(timestamp) = timestamp_column(contract) else {
            return Err(QueryError::BadCursor);
        };
        row_key::validate(locator.type_id, &locator.identity).map_err(QueryError::BadLocator)?;
        if locator.row_ordinal >= rows {
            return Err(QueryError::BadLocator(format!(
                "invalid detail_locator row_ordinal {}: type_id {} has {rows} rows in segment {}",
                locator.row_ordinal,
                locator.type_id,
                self.anchor.id(),
            )));
        }

        let ordinal = locate_row(segment, contract, timestamp, locator, sink)?;
        let row = read_row(segment, contract, ordinal, sink)?;
        let dictionary = chunk_dictionary(segment, &[(ordinal, row.clone())])?;
        let previous = self.previous_readings(segment, contract, timestamp, sink)?;
        let elapsed = previous
            .as_ref()
            .and_then(|before| locator.at.checked_sub(before.at))
            .filter(|elapsed| *elapsed > 0);
        let before = previous.as_ref().and_then(|before| {
            let changed_process = contract
                .column("starttime")
                .is_some_and(|column| row.get(column.name) != before.values.get(column.name));
            (!changed_process).then_some(&before.values)
        });
        let users = ProcessUsers::load(segment, contract, sink)?;
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }

        let mut fields = Map::new();
        for column in contract.columns {
            let stored = row.get(column.name);
            let exact_plan_calls = matches!(locator.type_id, 1_003_001 | 1_004_001 | 1_018_001)
                && column.name == "calls";
            let value = if column.class == ColumnClass::Cumulative && !exact_plan_calls {
                rate(stored, before, column.name, elapsed)
            } else {
                stored.map_or(Ok(Value::Null), |value| cell(value, &dictionary))?
            };
            fields.insert(column.name.to_owned(), value);
        }
        if locator.section == "os_process" {
            fields.insert(
                "user".to_owned(),
                users
                    .name_for(&row, "uid")
                    .map_or(Value::Null, |name| json!(name)),
            );
            fields.insert(
                "effective_user".to_owned(),
                users
                    .name_for(&row, "euid")
                    .map_or(Value::Null, |name| json!(name)),
            );
            fields.insert("cpu_time_ticks".to_owned(), scheduled_ticks(&row));
        }
        if locator.section == "pg_store_plans" {
            fields.insert(
                "calls_per_second".to_owned(),
                rate(row.get("calls"), before, "calls", elapsed),
            );
        }
        fields.insert("at".to_owned(), Value::String(locator.at.to_string()));
        crate::events::label_event_fields(&locator.section, &mut fields);
        normalize_detail_text(&locator.section, &mut fields)?;
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        Ok(RowDetailResult {
            section: locator.section.clone(),
            at: locator.at,
            fields,
        })
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let mut detail = self.execute(sink)?;
        detail.fields.remove("at");
        if !sink.cancelled() {
            sink.record(record(json!({
                "record": "row_detail",
                "section": detail.section,
                "at": detail.at.to_string(),
                "fields": detail.fields,
            }))?);
        }
        Ok(())
    }

    fn previous_readings(
        &self,
        anchor: &Segment,
        contract: &'static TypeContract,
        timestamp: &'static str,
        sink: &dyn QuerySink,
    ) -> Result<Option<PreviousReadings>, QueryError> {
        let locator = &self.request.locator;
        if !contract
            .columns
            .iter()
            .any(|column| column.class == ColumnClass::Cumulative)
        {
            return Ok(None);
        }
        let mut projection = row_key::identity_columns(contract).collect::<Vec<_>>();
        projection.extend(
            contract
                .columns
                .iter()
                .filter(|column| column.class == ColumnClass::Cumulative)
                .map(|column| column.name),
        );
        if contract.column("starttime").is_some() {
            projection.push("starttime");
        }
        projection.push(timestamp);
        projection.sort_unstable();
        projection.dedup();

        let mut candidates = std::iter::once(&self.anchor)
            .chain(
                self.other_segments
                    .iter()
                    .filter(|segment| segment.id() <= self.anchor.id()),
            )
            .filter(|segment| {
                segment
                    .sections()
                    .iter()
                    .any(|section| section.type_id == locator.type_id)
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|segment| Reverse((segment.max_ts(), segment.id())));

        let mut previous: Option<PreviousReadings> = None;
        for descriptor in candidates {
            if previous
                .as_ref()
                .is_some_and(|selected| descriptor.max_ts() < selected.at)
            {
                break;
            }
            if descriptor.id() == self.anchor.id() {
                scan_predecessor(anchor, locator, timestamp, &projection, sink, &mut previous)?;
            } else {
                let segment = self.dataset.open(descriptor)?;
                scan_predecessor(
                    &segment,
                    locator,
                    timestamp,
                    &projection,
                    sink,
                    &mut previous,
                )?;
            }
        }
        Ok(previous)
    }
}

struct PreviousReadings {
    at: i64,
    values: BTreeMap<&'static str, Cell>,
}

fn scan_predecessor(
    segment: &Segment,
    locator: &DetailLocator,
    timestamp: &'static str,
    projection: &[&'static str],
    sink: &dyn QuerySink,
    previous: &mut Option<PreviousReadings>,
) -> Result<(), QueryError> {
    segment.visit_rows(
        locator.type_id,
        projection,
        0,
        usize::MAX,
        |_ordinal, row| {
            if sink.cancelled() {
                return false;
            }
            let Some(stored) = row_timestamp(&row, timestamp) else {
                return true;
            };
            if stored >= locator.at {
                return true;
            }
            if row_key::identity(locator.type_id, &row)
                .is_ok_and(|identity| identity == locator.identity)
            {
                if previous
                    .as_ref()
                    .is_some_and(|selected| selected.at >= stored)
                {
                    return true;
                }
                *previous = Some(PreviousReadings {
                    at: stored,
                    values: projection
                        .iter()
                        .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
                        .collect(),
                });
            }
            true
        },
    )?;
    if sink.cancelled() {
        return Err(QueryError::Cancelled);
    }
    Ok(())
}

fn locate_row(
    segment: &Segment,
    contract: &'static TypeContract,
    timestamp: &'static str,
    locator: &DetailLocator,
    sink: &dyn QuerySink,
) -> Result<u64, QueryError> {
    let mut projection = vec![timestamp];
    projection.extend(row_key::identity_columns(contract));
    let mut resolved = None;
    let mut hinted = None;
    let mut failure = None;
    let equivalent_duplicates = contract.semantics == Semantics::EventStream;
    segment.visit_rows(
        locator.type_id,
        &projection,
        0,
        usize::MAX,
        |ordinal, row| {
            if sink.cancelled() {
                return false;
            }
            if row_timestamp(&row, timestamp) != Some(locator.at) {
                return true;
            }
            match row_key::identity(locator.type_id, &row) {
                Ok(identity) if identity == locator.identity => {
                    if ordinal == locator.row_ordinal {
                        hinted = Some(ordinal);
                    }
                    if resolved.is_none() {
                        resolved = Some(ordinal);
                        true
                    } else if equivalent_duplicates {
                        true
                    } else {
                        failure = Some(QueryError::BadLocator(format!(
                            "detail_locator identity is not unique for segment_id={}, type_id={}, at={}",
                            locator.segment_id, locator.type_id, locator.at,
                        )));
                        false
                    }
                }
                Ok(_) => true,
                Err(error) => {
                    failure = Some(QueryError::BadLocator(error));
                    false
                }
            }
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if sink.cancelled() {
        return Err(QueryError::Cancelled);
    }
    hinted.or(resolved).ok_or_else(|| {
        QueryError::BadLocator(format!(
            "no stored row matches detail_locator segment_id={}, type_id={}, at={} and identity",
            locator.segment_id, locator.type_id, locator.at,
        ))
    })
}

fn read_row(
    segment: &Segment,
    contract: &'static TypeContract,
    ordinal: u64,
    sink: &dyn QuerySink,
) -> Result<Row, QueryError> {
    let projection = contract
        .columns
        .iter()
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let mut selected = None;
    segment.visit_rows(
        contract.type_id.get(),
        &projection,
        ordinal,
        1,
        |_stored, row| {
            selected = Some(row);
            false
        },
    )?;
    if sink.cancelled() {
        return Err(QueryError::Cancelled);
    }
    selected.ok_or_else(|| {
        QueryError::BadLocator("detail_ref does not identify one recorded row".to_owned())
    })
}

fn timestamp_column(contract: &'static TypeContract) -> Option<&'static str> {
    contract
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .map(|column| column.name)
}

fn row_timestamp(row: &Row, column: &'static str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(stored)) => Some(*stored),
        _ => None,
    }
}

#[derive(Default)]
struct ProcessUsers {
    names: HashMap<(u8, u32), String>,
}

impl ProcessUsers {
    fn load(
        segment: &Segment,
        contract: &'static TypeContract,
        sink: &dyn QuerySink,
    ) -> Result<Self, QueryError> {
        if contract.name != "os_process" || segment.rows_of(PROCESS_USER_TYPE_ID).is_none() {
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
                if sink.cancelled() {
                    return false;
                }
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
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        if encoded.len() > MAX_PROCESS_USERS {
            return Err(unreadable("os_user exceeds the per-segment mapping limit"));
        }
        let dictionary = segment.dictionary_for(&ids)?;
        if let Some(id) = ids
            .iter()
            .copied()
            .find(|id| dictionary.resolve(*id).is_none())
        {
            return Err(unreadable(format!("unresolved dictionary id {id}")));
        }
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

    fn name_for<'a>(&'a self, row: &Row, uid_column: &str) -> Option<&'a str> {
        let (Some(Cell::U32(scope)), Some(Cell::U32(uid))) =
            (row.get("scope"), row.get(uid_column))
        else {
            return None;
        };
        let scope = u8::try_from(*scope).ok()?;
        self.names.get(&(scope, *uid)).map(String::as_str)
    }
}

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

fn rate(
    stored: Option<&Cell>,
    before: Option<&BTreeMap<&'static str, Cell>>,
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
    let value = delta / seconds;
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "integer counter deltas are converted only after exact subtraction"
)]
fn counter_delta(now: &Cell, earlier: &Cell) -> Option<f64> {
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
            return (delta >= 0.0 && delta.is_finite()).then_some(delta);
        }
        _ => return None,
    };
    (exact >= 0).then_some(exact as f64)
}

fn normalize_detail_text(section: &str, fields: &mut Map<String, Value>) -> Result<(), QueryError> {
    for (field, value) in fields {
        if row_key::is_detail_text(section, field) && !value.is_null() {
            *value = stable_text(std::mem::take(value)).map_err(|error| {
                unreadable(format!(
                    "internal error: {section}.{field} is not stored text: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

fn stable_text(value: Value) -> Result<Value, &'static str> {
    match value {
        Value::String(stored_text) => Ok(json!({
            "full_len": stored_text.len().to_string(),
            "sha256": null,
            "stored_text": stored_text,
            "truncated": false,
        })),
        Value::Object(object) if object.get("representation") == Some(&json!("text")) => {
            let stored_text = object.get("stored_text").ok_or("missing stored_text")?;
            let full_len = object.get("full_len").ok_or("missing full_len")?;
            let truncated = object.get("truncated").ok_or("missing truncated")?;
            let sha256 = object.get("sha256").ok_or("missing sha256")?;
            Ok(json!({
                "full_len": full_len,
                "sha256": sha256,
                "stored_text": stored_text,
                "truncated": truncated,
            }))
        }
        _ => Err("expected a UTF-8 string"),
    }
}

fn unreadable(message: impl Into<String>) -> QueryError {
    QueryError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

#[cfg(test)]
mod tests;
