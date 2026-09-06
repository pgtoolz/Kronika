//! Shared ordered Heatmap query, execution, and result.

mod execution;
mod query;
mod result;

pub use execution::{HeatmapError, ValidatedHeatmapQuery, execute_heatmap_batch};
pub(crate) use execution::{PreparedHeatmap, prepare};
pub use query::{
    DEFAULT_TOP, HeatmapBatchQuery, HeatmapItemQuery, HeatmapView, MAX_FIELDS, MAX_TOP,
    NormalizedRanking,
};
pub use result::{
    CoverageState, HeatmapBand, HeatmapBatchResult, HeatmapCoverage, HeatmapEntity, HeatmapGrid,
    HeatmapGroup, HeatmapInterval, HeatmapItemResult, HeatmapSummary, NamedValues,
};

/// Validate one heatmap request without opening its recorded-data source.
///
/// # Errors
///
/// Returns the same semantic error that [`crate::execute`] would return before
/// accessing the query dataset.
pub fn validate_heatmap_request(
    query: HeatmapBatchQuery,
) -> Result<ValidatedHeatmapQuery, crate::QueryError> {
    execution::validate_request(query)
}
