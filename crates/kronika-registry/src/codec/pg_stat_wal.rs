//! Type `1_007_001` / `1_007_002`: `pg_stat_wal`.
//!
//! Cluster-wide WAL counters. `1_007_002` removes PG18 write/sync fields.

use crate::{Section, Ts};

/// Type `1_007_001`: `pg_stat_wal` on PG 14-17.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_007_001,
    name = "pg_stat_wal",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatWalV1 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated (`numeric` in the view, stored as `i64`).
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
    /// Times WAL data was written to disk because the WAL buffers filled.
    #[column(c, unit = count)]
    pub wal_buffers_full: i64,
    /// Times WAL buffers were written out to disk via `XLogWrite`.
    #[column(c, unit = count)]
    pub wal_write: i64,
    /// Times WAL files were synced to disk via `issue_xlog_fsync`.
    #[column(c, unit = count)]
    pub wal_sync: i64,
    /// Time spent writing WAL to disk, ms; `0.0` without `track_wal_io_timing`.
    #[column(c, unit = milliseconds)]
    pub wal_write_time: f64,
    /// Time spent syncing WAL to disk, ms; `0.0` without `track_wal_io_timing`.
    #[column(c, unit = milliseconds)]
    pub wal_sync_time: f64,
    /// Time of the last `pg_stat_wal` reset; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

/// Type `1_007_002`: `pg_stat_wal` on PG 18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_007_002,
    name = "pg_stat_wal",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatWalV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated (`numeric` in the view, stored as `i64`).
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
    /// Times WAL data was written to disk because the WAL buffers filled.
    #[column(c, unit = count)]
    pub wal_buffers_full: i64,
    /// Time of the last `pg_stat_wal` reset; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_wal.rs"]
mod tests;
