//! Type `1_020_001`: current regular-file size in `pg_wal`.

use crate::{Section, Ts};

/// Type `1_020_001`: summed size of files returned by `pg_ls_waldir()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_020_001,
    name = "pg_wal_storage",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct PgWalStorage {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Total size of the regular files visible in `pg_wal`.
    #[column(g, unit = bytes)]
    pub wal_files_bytes: i64,
}

#[cfg(test)]
mod tests;
