//! Type `1_008_001`: `pg_stat_archiver`.
//!
//! WAL archiver singleton.

use crate::{Section, StrId, Ts};

/// Type `1_008_001`: `pg_stat_archiver`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_008_001,
    name = "pg_stat_archiver",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgStatArchiver {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// WAL files successfully archived.
    #[column(c, unit = count)]
    pub archived_count: i64,
    /// Last successfully archived WAL file; `None` until the first archive.
    #[column(l)]
    pub last_archived_wal: Option<StrId>,
    /// Time of the last successful archive.
    #[column(g, unit = microseconds)]
    pub last_archived_time: Option<Ts>,
    /// Failed archive attempts.
    #[column(c, unit = count)]
    pub failed_count: i64,
    /// WAL file of the last failed attempt; `None` until the first failure.
    #[column(l)]
    pub last_failed_wal: Option<StrId>,
    /// Time of the last failed attempt.
    #[column(g, unit = microseconds)]
    pub last_failed_time: Option<Ts>,
    /// Time of the last statistics reset; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_archiver.rs"]
mod tests;
