//! Type `1_015_001`: `pg_stat_statements_info` on extension 1.9 and later.

use crate::{Section, Ts};

/// Type `1_015_001`: module-level `pg_stat_statements` counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_015_001,
    name = "pg_stat_statements_info",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatStatementsInfo {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Least-executed statement entries evicted after the table filled.
    #[column(c, unit = count)]
    pub dealloc: i64,
    /// Time when all statement statistics were last reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Ts,
}

#[cfg(test)]
mod tests;
