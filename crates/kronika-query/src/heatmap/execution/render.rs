//! Heatmap labels, identities, and cell rendering.

use std::cmp::Ordering;

use kronika_reader::{Cell, Dictionary};
use kronika_registry::contract;
use serde_json::Value;

use super::{HeatmapError, IDENTITY_ALIASES, ItemSpec, LabelCutoff, RenderedIds, StoredLabel};

use crate::heatmap::result::NamedValues;
use crate::render::cell;

pub(super) fn compare_totals(left: Option<&f64>, right: Option<&f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.partial_cmp(left).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub(super) fn group_order(totals: &[Option<f64>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..totals.len()).collect();
    order.sort_by(|left, right| {
        compare_totals(totals[*left].as_ref(), totals[*right].as_ref())
            .then_with(|| left.cmp(right))
    });
    order
}

pub(super) fn ranking_reaches_cutoff(total: Option<f64>, cutoff: LabelCutoff) -> bool {
    match cutoff {
        LabelCutoff::Value(cutoff) if !cutoff.is_finite() => true,
        LabelCutoff::Value(cutoff) => {
            total.is_some_and(|total| !total.is_finite() || total >= cutoff)
        }
        LabelCutoff::Null => true,
    }
}

pub(super) fn summary_total(values: impl Iterator<Item = f64>, additive: bool) -> Option<f64> {
    values.fold(None, |current, value| {
        Some(match current {
            Some(current) if !additive => current.max(value),
            Some(current) => current + value,
            None => value,
        })
    })
}

pub(super) fn identity_object(type_id: u32, values: Vec<Value>) -> NamedValues {
    let names = contract(type_id)
        .map(|contract| contract.identity)
        .unwrap_or_default();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let name = names.get(index).map_or_else(
                || format!("value_{index}"),
                |name| public_identity_name(name).to_owned(),
            );
            (name, value)
        })
        .collect()
}

pub(super) fn public_identity_name(name: &str) -> &str {
    IDENTITY_ALIASES
        .iter()
        .find(|(recorded, _public)| recorded == &name)
        .map_or(name, |(_recorded, public)| public)
}

pub(super) fn labels_object(
    spec: &ItemSpec,
    labels: &[Option<StoredLabel>],
    label_slots: &[usize],
    dictionary: &RenderedIds,
    index: usize,
) -> Result<NamedValues, HeatmapError> {
    spec.labels
        .iter()
        .zip(label_slots)
        .map(|(name, slot)| {
            let stored = &labels[*slot];
            let value = stored.as_ref().map_or(Ok(Value::Null), |stored| {
                render_cell(&stored.value, stored.segment_slot, dictionary, index)
            })?;
            Ok((name.clone(), value))
        })
        .collect()
}

pub(super) fn render_cells(
    cells: &[Cell],
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Vec<Value>, HeatmapError> {
    cells
        .iter()
        .map(|stored| render_cell(stored, segment_slot, dictionary, index))
        .collect()
}

fn render_cell(
    stored: &Cell,
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Value, HeatmapError> {
    if let Cell::StrId(id) = stored {
        return dictionary
            .get(&(segment_slot, *id))
            .cloned()
            .ok_or_else(|| HeatmapError::failure(index, format!("unresolved dictionary id {id}")));
    }
    cell(stored, &Dictionary::default()).map_err(|error| HeatmapError::storage(index, error))
}
