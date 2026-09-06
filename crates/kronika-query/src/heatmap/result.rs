//! Typed Heatmap result shared by recorded-data adapters.

use std::collections::BTreeMap;

use kronika_registry::{ColumnClass, Unit};
use serde::Serialize;
use serde_json::Value;

use super::query::NormalizedRanking;
use crate::row_key::{DetailLocator, serialize_decimal};

/// Stable name-to-value object used for identities and labels.
pub type NamedValues = BTreeMap<String, Value>;

/// Results in exact requested ranking order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapBatchResult {
    /// One result per requested item, including exact duplicates.
    pub results: Vec<HeatmapItemResult>,
}

/// One ranked metric fold and optional time grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapItemResult {
    /// Normalized input selection.
    pub ranking: NormalizedRanking,
    /// Whether the requested window carried eligible rows.
    pub coverage: HeatmapCoverage,
    /// Registry quantity class shared by the selected fields.
    #[serde(serialize_with = "serialize_class")]
    pub class: ColumnClass,
    /// Reduction used for ranked row and band summaries.
    pub summary: HeatmapSummary,
    /// Registry unit shared by the selected fields, when declared.
    #[serde(serialize_with = "serialize_optional_unit")]
    pub unit: Option<Unit>,
    /// Ranked entities in semantic order.
    pub entities: Vec<HeatmapEntity>,
    /// Authoritative total across every eligible entity.
    pub totals_total: Option<f64>,
    /// Total outside the retained top entities.
    pub others_total: Option<f64>,
    /// Complete eligible entity count before top-N truncation.
    #[serde(serialize_with = "serialize_decimal")]
    pub entity_count: u64,
    /// Rows whose physical timestamp order moved backwards.
    #[serde(serialize_with = "serialize_decimal")]
    pub out_of_order: u64,
    /// Bucketed output for a grid view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid: Option<HeatmapGrid>,
}

/// Whether a ranking window contained eligible data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageState {
    /// At least one eligible row was observed.
    Data,
    /// No eligible rows were observed.
    NoData,
}

/// Meaning of a heatmap's ranked row and band summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatmapSummary {
    /// Additive changes over the requested window.
    Sum,
    /// Maximum recorded values.
    Max,
    /// Average contribution across recorded process snapshots.
    Mean,
}

/// Coverage facts for one ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HeatmapCoverage {
    /// Data/no-data state.
    pub state: CoverageState,
    /// Eligible physical rows in the selected window.
    #[serde(serialize_with = "serialize_decimal")]
    pub window_rows: u64,
}

/// One retained ranked entity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapEntity {
    /// Stable public identity.
    pub identity: NamedValues,
    /// Display fields captured from the representative row.
    pub labels: NamedValues,
    pub(crate) detail_locator: DetailLocator,
    /// Folded total used for ranking.
    pub total: Option<f64>,
    /// Per-column values for a grid view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cells: Option<Vec<Option<f64>>>,
}

impl HeatmapEntity {
    /// Encode the representative row as an opaque detail reference.
    ///
    /// # Errors
    ///
    /// Returns an explanation when the internal locator is invalid.
    pub fn detail_ref(&self) -> Result<String, String> {
        self.detail_locator.detail_ref()
    }
}

/// Grid labels, intervals, groups, totals, and remainder.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapGrid {
    /// Entity label fields in output order.
    pub label_names: Vec<String>,
    /// Aggregation fields in output order.
    pub group_names: Vec<String>,
    /// Equal time intervals with inclusive endpoints.
    pub intervals: Vec<HeatmapInterval>,
    /// Aggregated entity groups.
    pub groups: Vec<HeatmapGroup>,
    /// All-entity band.
    pub totals: HeatmapBand,
    /// Band outside the retained entities.
    pub others: HeatmapBand,
}

/// One grid interval with inclusive endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HeatmapInterval {
    /// Inclusive interval start.
    #[serde(serialize_with = "serialize_decimal")]
    pub start: i64,
    /// Inclusive interval end.
    #[serde(serialize_with = "serialize_decimal")]
    pub end: i64,
}

/// One aggregated entity group in a grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapGroup {
    /// Group values in `group_names` order.
    pub values: Vec<Value>,
    /// Retained entities represented by this group.
    pub members: u32,
    /// Folded total for the group.
    pub total: Option<f64>,
    /// Per-column group values.
    pub cells: Vec<Option<f64>>,
}

/// Total and per-column values for one grid band.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatmapBand {
    /// Folded band total.
    pub total: Option<f64>,
    /// Per-column band values.
    pub cells: Vec<Option<f64>>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the field by reference"
)]
fn serialize_class<S>(value: &ColumnClass, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(value.code())
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the optional field by reference"
)]
fn serialize_optional_unit<S>(value: &Option<Unit>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(value.code()),
        None => serializer.serialize_none(),
    }
}
