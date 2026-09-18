//! Types `1_012_004` through `1_012_006`: `pg_stat_progress_vacuum`.
//!
//! One row per backend running `VACUUM`; absence means no active rows. The
//! view changed incompatibly in PG17 and PG18, so each exact server shape has
//! its own codec. `schemaname`/`relname` are resolved from `pg_class` at
//! collection time and absent when the relation belongs to a database the
//! collector's connection cannot see or was dropped since; `relid` stays the
//! identity either way.

use crate::{Section, StrId, Ts};

/// Type `1_012_004`: `pg_stat_progress_vacuum`, PG10-16 layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_012_004,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple capacity.
    #[column(g, unit = count)]
    pub max_dead_tuples: i64,
    /// Dead tuples collected in the current cycle.
    #[column(g, unit = count)]
    pub num_dead_tuples: i64,
}

/// Type `1_012_005`: `pg_stat_progress_vacuum`, PG17 layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_012_005,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    #[column(g, unit = bytes)]
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    #[column(g, unit = bytes)]
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    #[column(g, unit = count)]
    pub num_dead_item_ids: i64,
    /// Indexes to process in this cycle.
    #[column(g, unit = count)]
    pub indexes_total: i64,
    /// Indexes processed in this cycle.
    #[column(g, unit = count)]
    pub indexes_processed: i64,
}

/// Type `1_012_006`: `pg_stat_progress_vacuum`, PG18+ layout.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_012_006,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV3 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    #[column(g, unit = bytes)]
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    #[column(g, unit = bytes)]
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    #[column(g, unit = count)]
    pub num_dead_item_ids: i64,
    /// Indexes to process in this cycle.
    #[column(g, unit = count)]
    pub indexes_total: i64,
    /// Indexes processed in this cycle.
    #[column(g, unit = count)]
    pub indexes_processed: i64,
    /// Cost-delay sleep time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub delay_time: f64,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_progress_vacuum.rs"]
mod tests;
