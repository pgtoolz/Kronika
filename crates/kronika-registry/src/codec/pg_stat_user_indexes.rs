//! Type `1_014_003`..`1_014_004`: `pg_stat_user_indexes`.
//!
//! Per-index statistics, one row per selected index per database. In PG 10-18
//! the column set grows once: `last_idx_scan` arrives in PG16. PG17 and PG18 add
//! nothing to `pg_stat_all_indexes`, so the source maps those catalog layouts to
//! two layout versions.
//!
//! Each layout merges `pg_statio_user_indexes` (the buffer-I/O counters), the
//! `pg_index` flags (`indisunique`/`indisprimary`/`indisvalid`/`indisexclusion`/
//! `indisready`), the access method name from `pg_am`, and `pg_get_indexdef` into
//! the same row. `indexdef` can be `None` if the index is dropped concurrently.

use crate::{Section, StrId, Ts};

/// Type `1_014_004`: `pg_stat_user_indexes` on PG 16-18 (V1 plus `last_idx_scan`).
///
/// One row per selected index per database. `last_idx_scan` is `None` when the
/// index has never been scanned. Every column is an integer, `StrId`, or `bool`,
/// so the layout derives `Eq`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_014_004,
    name = "pg_stat_user_indexes",
    semantics = snapshot_full,
    sort_key("datid", "indexrelid", "ts"),
    identity("datid", "indexrelid")
)]
pub struct PgStatUserIndexesV2 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Index oid.
    #[column(l)]
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Index name.
    #[column(l)]
    pub indexrelname: StrId,
    /// Effective tablespace oid.
    #[column(l)]
    pub tablespace_oid: u32,
    /// Effective tablespace name; `None` when catalog label resolution is missing.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Index scans.
    #[column(c, unit = count)]
    pub idx_scan: i64,
    /// Index entries returned by scans.
    #[column(c, unit = count)]
    pub idx_tup_read: i64,
    /// Live table rows fetched by simple index scans using this index.
    #[column(c, unit = count)]
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes (`pg_relation_size(indexrelid)`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// Last index scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_idx_scan: Option<Ts>,
    /// Whether the index enforces uniqueness.
    #[column(l)]
    pub indisunique: bool,
    /// Whether the index is a primary key.
    #[column(l)]
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    #[column(l)]
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    #[column(l)]
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    #[column(l)]
    pub indisready: bool,
    /// Access method name (`btree`, `hash`, `gin`, ...).
    #[column(l)]
    pub amname: StrId,
    /// `pg_get_indexdef` reconstruction; `None` after a concurrent index drop.
    #[column(l)]
    pub indexdef: Option<StrId>,
    /// Shared-buffer misses for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_hit: i64,
}

/// Type `1_014_003`: `pg_stat_user_indexes` on PG 10-15 (base layout, no
/// `last_idx_scan`). Column meanings match [`PgStatUserIndexesV2`] for fields
/// present in this layout.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_014_003,
    name = "pg_stat_user_indexes",
    semantics = snapshot_full,
    sort_key("datid", "indexrelid", "ts"),
    identity("datid", "indexrelid")
)]
pub struct PgStatUserIndexesV1 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Index oid.
    #[column(l)]
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Index name.
    #[column(l)]
    pub indexrelname: StrId,
    /// Effective tablespace oid.
    #[column(l)]
    pub tablespace_oid: u32,
    /// Effective tablespace name; `None` when catalog label resolution is missing.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Index scans.
    #[column(c, unit = count)]
    pub idx_scan: i64,
    /// Index entries returned by scans.
    #[column(c, unit = count)]
    pub idx_tup_read: i64,
    /// Live table rows fetched by simple index scans using this index.
    #[column(c, unit = count)]
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes (`pg_relation_size(indexrelid)`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// Whether the index enforces uniqueness.
    #[column(l)]
    pub indisunique: bool,
    /// Whether the index is a primary key.
    #[column(l)]
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    #[column(l)]
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    #[column(l)]
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    #[column(l)]
    pub indisready: bool,
    /// Access method name (`btree`, `hash`, `gin`, ...).
    #[column(l)]
    pub amname: StrId,
    /// `pg_get_indexdef` reconstruction; `None` after a concurrent index drop.
    #[column(l)]
    pub indexdef: Option<StrId>,
    /// Shared-buffer misses for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_hit: i64,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_user_indexes.rs"]
mod tests;
