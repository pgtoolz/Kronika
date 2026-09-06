//! Type `1_016_001`: a discovered `pg_store_plans_info(dealloc, stats_reset)` view.

use crate::{Section, Ts};

/// Type `1_016_001`: module-level counters from the discovered info view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_016_001,
    name = "pg_store_plans_info",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStorePlansInfo {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Least-executed plan entries evicted after the table filled.
    #[column(c, unit = count)]
    pub dealloc: i64,
    /// Time when all plan statistics were last reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Ts,
}

#[cfg(test)]
mod tests;
