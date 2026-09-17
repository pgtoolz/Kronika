//! Find semantic boundary cohorts, then retain rows without changing their order.

use std::collections::BTreeMap;

use arrow_array::{Array as _, BooleanArray, Int64Array, RecordBatch};
use arrow_select::filter::filter_record_batch;
use kronika_reader::{Reader, SegmentRef};
use kronika_registry::{CodecError, ColumnType, Semantics, TypeContract, contract};

use super::{MicrosRange, SliceError, is_dictionary};

#[derive(Debug)]
pub(super) struct TypeSelection {
    pub(super) contract: &'static TypeContract,
    pub(super) has_requested_rows: bool,
    boundary: Boundary,
}

#[derive(Debug)]
enum Boundary {
    None,
    AllContext,
    // Keep the complete nearest snapshot on each side, even when the cohort
    // spans physical segments. The first pass finds these timestamps globally.
    Cohort {
        before: Option<i64>,
        after: Option<i64>,
    },
}

pub(super) fn select_boundaries(
    reader: &Reader,
    references: &[SegmentRef],
    range: MicrosRange,
) -> Result<BTreeMap<u32, TypeSelection>, SliceError> {
    let mut selections = BTreeMap::new();
    for reference in references {
        let segment = reader.open_segment(reference)?;
        for type_id in segment.type_ids() {
            if is_dictionary(type_id) {
                continue;
            }
            let contract = contract(type_id).ok_or(SliceError::UnsliceableType { type_id })?;
            let timestamp =
                timestamp_column(contract).ok_or(SliceError::UnsliceableType { type_id })?;
            let selection = selections.entry(type_id).or_insert_with(|| TypeSelection {
                contract,
                has_requested_rows: false,
                boundary: match contract.semantics {
                    Semantics::EventStream => Boundary::None,
                    Semantics::Changed | Semantics::OnChange => Boundary::AllContext,
                    Semantics::SnapshotFull | Semantics::ConditionalFull => Boundary::Cohort {
                        before: None,
                        after: None,
                    },
                },
            });
            let mut callback_error = None;
            segment.visit_batches(
                type_id,
                Some(&[timestamp]),
                0,
                usize::MAX,
                |_ordinal, batch| {
                    let values = match timestamps(&batch, contract) {
                        Ok(values) => values,
                        Err(problem) => {
                            callback_error = Some(problem);
                            return false;
                        }
                    };
                    for value in values.values() {
                        observe_timestamp(selection, *value, range);
                    }
                    true
                },
            )?;
            if let Some(problem) = callback_error {
                return Err(problem);
            }
        }
    }
    Ok(selections)
}

fn timestamp_column(contract: &TypeContract) -> Option<&'static str> {
    let mut timestamps = contract
        .columns
        .iter()
        .filter(|column| column.class == kronika_registry::ColumnClass::Timestamp);
    let column = timestamps.next()?;
    (timestamps.next().is_none() && column.ty == ColumnType::Ts && !column.nullable)
        .then_some(column.name)
}

fn timestamps<'a>(
    batch: &'a RecordBatch,
    contract: &TypeContract,
) -> Result<&'a Int64Array, SliceError> {
    let name = timestamp_column(contract).ok_or(SliceError::UnsliceableType {
        type_id: contract.type_id.get(),
    })?;
    let values = batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<Int64Array>())
        .filter(|values| values.null_count() == 0)
        .ok_or(SliceError::InvalidBatch {
            type_id: contract.type_id.get(),
            column: name,
        })?;
    Ok(values)
}

fn observe_timestamp(selection: &mut TypeSelection, value: i64, range: MicrosRange) {
    if range.in_request(value) {
        selection.has_requested_rows = true;
        return;
    }
    match &mut selection.boundary {
        Boundary::Cohort { before, .. } if range.before(value) => {
            *before = Some(before.map_or(value, |current| current.max(value)));
        }
        Boundary::Cohort { after, .. } if range.after(value) => {
            *after = Some(after.map_or(value, |current| current.min(value)));
        }
        Boundary::None | Boundary::AllContext | Boundary::Cohort { .. } => {}
    }
}

pub(super) fn retain_batch(
    batch: &RecordBatch,
    selection: &TypeSelection,
    range: MicrosRange,
) -> Result<Option<(RecordBatch, i64, i64)>, SliceError> {
    let ts = timestamps(batch, selection.contract)?;
    let mut mask = Vec::with_capacity(batch.num_rows());
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    for &value in ts.values() {
        let retain = if range.in_request(value) {
            true
        } else {
            match &selection.boundary {
                Boundary::None => false,
                Boundary::AllContext => range.before(value) || range.after(value),
                Boundary::Cohort { before, after } => {
                    *before == Some(value) || *after == Some(value)
                }
            }
        };
        mask.push(retain);
        if retain {
            min_ts = min_ts.min(value);
            max_ts = max_ts.max(value);
        }
    }
    if min_ts > max_ts {
        return Ok(None);
    }
    let retained =
        filter_record_batch(batch, &BooleanArray::from(mask)).map_err(CodecError::from)?;
    Ok(Some((retained, min_ts, max_ts)))
}
