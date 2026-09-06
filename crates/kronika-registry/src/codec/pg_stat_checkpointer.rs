//! Types `1_017_001` / `1_017_002`: `pg_stat_checkpointer`.
//!
//! The view exists from `PostgreSQL` 17. `PostgreSQL` 18 added completed
//! checkpoint and SLRU-buffer counters.

use crate::{Section, Ts};

/// Type `1_017_001`: `pg_stat_checkpointer` on `PostgreSQL` 17.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_017_001,
    name = "pg_stat_checkpointer",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatCheckpointerV1 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Checkpoints scheduled by timeout, including skipped checkpoints.
    #[column(c, unit = count)]
    pub num_timed: i64,
    /// Requested checkpoints performed.
    #[column(c, unit = count)]
    pub num_requested: i64,
    /// Restartpoints scheduled by timeout or a previous failed attempt.
    #[column(c, unit = count)]
    pub restartpoints_timed: i64,
    /// Requested restartpoints.
    #[column(c, unit = count)]
    pub restartpoints_req: i64,
    /// Restartpoints performed.
    #[column(c, unit = count)]
    pub restartpoints_done: i64,
    /// Time spent writing checkpoint and restartpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub write_time: f64,
    /// Time spent synchronizing checkpoint and restartpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub sync_time: f64,
    /// Shared buffers written during checkpoints and restartpoints.
    #[column(c, unit = count)]
    pub buffers_written: i64,
    /// Time of the last statistics reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

/// Type `1_017_002`: `pg_stat_checkpointer` on `PostgreSQL` 18.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_017_002,
    name = "pg_stat_checkpointer",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatCheckpointerV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Checkpoints scheduled by timeout, including skipped checkpoints.
    #[column(c, unit = count)]
    pub num_timed: i64,
    /// Requested checkpoints, including skipped checkpoints.
    #[column(c, unit = count)]
    pub num_requested: i64,
    /// Checkpoints performed.
    #[column(c, unit = count)]
    pub num_done: i64,
    /// Restartpoints scheduled by timeout or a previous failed attempt.
    #[column(c, unit = count)]
    pub restartpoints_timed: i64,
    /// Requested restartpoints.
    #[column(c, unit = count)]
    pub restartpoints_req: i64,
    /// Restartpoints performed.
    #[column(c, unit = count)]
    pub restartpoints_done: i64,
    /// Time spent writing checkpoint and restartpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub write_time: f64,
    /// Time spent synchronizing checkpoint and restartpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub sync_time: f64,
    /// Shared buffers written during checkpoints and restartpoints.
    #[column(c, unit = count)]
    pub buffers_written: i64,
    /// SLRU buffers written during checkpoints and restartpoints.
    #[column(c, unit = count)]
    pub slru_written: i64,
    /// Time of the last statistics reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

#[cfg(test)]
mod tests;
