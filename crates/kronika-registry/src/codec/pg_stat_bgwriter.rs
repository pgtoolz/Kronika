//! Types `1_006_001` / `1_006_002`: `pg_stat_bgwriter`.
//!
//! `PostgreSQL` 17 moved checkpoint counters into `pg_stat_checkpointer`, so
//! the remaining background-writer view has a separate layout.

use crate::{Section, Ts};

/// Type `1_006_001`: `pg_stat_bgwriter` on `PostgreSQL` 10-16.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_006_001,
    name = "pg_stat_bgwriter",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatBgwriterV1 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Scheduled checkpoints performed.
    #[column(c, unit = count)]
    pub checkpoints_timed: i64,
    /// Requested checkpoints performed.
    #[column(c, unit = count)]
    pub checkpoints_req: i64,
    /// Time spent writing checkpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub checkpoint_write_time: f64,
    /// Time spent synchronizing checkpoint files, milliseconds.
    #[column(c, unit = milliseconds)]
    pub checkpoint_sync_time: f64,
    /// Buffers written during checkpoints.
    #[column(c, unit = count)]
    pub buffers_checkpoint: i64,
    /// Buffers written by the background writer.
    #[column(c, unit = count)]
    pub buffers_clean: i64,
    /// Cleaning scans stopped by `bgwriter_lru_maxpages`.
    #[column(c, unit = count)]
    pub maxwritten_clean: i64,
    /// Buffers written directly by backends.
    #[column(c, unit = count)]
    pub buffers_backend: i64,
    /// Backend-issued fsync calls.
    #[column(c, unit = count)]
    pub buffers_backend_fsync: i64,
    /// Buffers allocated.
    #[column(c, unit = count)]
    pub buffers_alloc: i64,
    /// Time of the last statistics reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

/// Type `1_006_002`: `pg_stat_bgwriter` on `PostgreSQL` 17-18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_006_002,
    name = "pg_stat_bgwriter",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatBgwriterV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Buffers written by the background writer.
    #[column(c, unit = count)]
    pub buffers_clean: i64,
    /// Cleaning scans stopped by `bgwriter_lru_maxpages`.
    #[column(c, unit = count)]
    pub maxwritten_clean: i64,
    /// Buffers allocated.
    #[column(c, unit = count)]
    pub buffers_alloc: i64,
    /// Time of the last statistics reset.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

#[cfg(test)]
mod tests;
