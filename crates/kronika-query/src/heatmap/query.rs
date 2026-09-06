//! Normalized Heatmap queries.

use serde::Serialize;

use crate::{StatementScope, TimeRange};

/// Default number of retained ranked entities.
pub const DEFAULT_TOP: usize = 25;
/// Maximum number of retained ranked entities.
pub const MAX_TOP: usize = 500;
/// Maximum fields folded by one ranking item.
pub const MAX_FIELDS: usize = 4;

/// Ordered batch of Heatmap rankings over one half-open range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeatmapBatchQuery {
    /// Shared half-open recorded-time range.
    pub range: TimeRange,
    /// Ranking items in caller order; exact duplicates share execution.
    pub items: Vec<HeatmapItemQuery>,
}

/// One normalized ranking and its requested result view.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeatmapItemQuery {
    /// Section, metric fields, and retained entity count.
    pub ranking: NormalizedRanking,
    /// Ranking-only or bucketed-grid output.
    pub view: HeatmapView,
    /// Which statements a `pg_stat_statements` ranking includes.
    pub scope: StatementScope,
}

/// Stable ranking selection exposed in typed results.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct NormalizedRanking {
    /// Registry logical-section name.
    pub section: String,
    /// One to four compatible numeric fields folded together.
    pub fields: Vec<String>,
    /// Maximum ranked entities retained.
    pub top: usize,
}

/// Requested result materialization for one ranking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HeatmapView {
    /// Totals and ranked entities without time buckets.
    RankingOnly,
    /// Totals, ranked entities, and a fixed-width time grid.
    Grid {
        /// Number of equal half-open time columns.
        columns: usize,
        /// Optional public fields used to aggregate ranked entities.
        group: Vec<String>,
        /// Optional exact physical layout restriction.
        type_id: Option<u32>,
    },
}

impl HeatmapView {
    pub(crate) const fn type_id(&self) -> Option<u32> {
        match self {
            Self::RankingOnly => None,
            Self::Grid { type_id, .. } => *type_id,
        }
    }

    pub(crate) fn groups(&self) -> &[String] {
        match self {
            Self::RankingOnly => &[],
            Self::Grid { group, .. } => group,
        }
    }

    pub(crate) const fn columns(&self) -> usize {
        match self {
            Self::RankingOnly => 1,
            Self::Grid { columns, .. } => *columns,
        }
    }
}
