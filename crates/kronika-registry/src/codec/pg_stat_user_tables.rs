//! Type `1_013_005`..`1_013_008`: `pg_stat_user_tables`.
//!
//! Per-table statistics, one row per selected table per database. In PG 10-18
//! the column set only grows: `n_ins_since_vacuum` arrives in PG13;
//! `n_tup_newpage_upd` plus the `last_seq_scan`/`last_idx_scan` timestamps in
//! PG16; the four cumulative vacuum/analyze timing columns in PG18. The source
//! maps those catalog layouts to four layout versions.
//!
//! Each layout merges `pg_statio_user_tables` (the buffer-I/O counters) and the
//! `pg_class` wraparound ages (`xid_age`, `mxid_age`) into the same row. `idx_*`
//! columns are `None` when the table has no indexes; `toast_*` columns are
//! `None` when it has no TOAST relation; `last_*` timestamps are `None` when the
//! event never happened. `xid_age` and `mxid_age` are `None` for partitioned
//! parents, whose invalid xid/mxid sentinels have no meaningful age.

use crate::{Section, StrId, Ts};

/// Type `1_013_008`: `pg_stat_user_tables` on PG 18 (V3 plus the four cumulative
/// vacuum/analyze timing columns).
///
/// One row per selected table per database. `idx_*` columns are `None` when the
/// table has no indexes; `toast_*` columns are `None` when it has no TOAST
/// relation; `last_*` timestamps are `None` when the event never happened. The
/// `total_*_time` columns are `f64` milliseconds, so the layout drops `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_013_008,
    name = "pg_stat_user_tables",
    semantics = snapshot_full,
    sort_key("datid", "relid", "ts"),
    identity("datid", "relid")
)]
pub struct PgStatUserTablesV4 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Table oid.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Effective tablespace oid; `None` for a storage-less partitioned parent.
    #[column(l)]
    pub tablespace_oid: Option<u32>,
    /// Effective tablespace name; `None` for a storage-less parent or missing label.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Sequential scans.
    #[column(c, unit = count)]
    pub seq_scan: i64,
    /// Live rows fetched by sequential scans.
    #[column(c, unit = count)]
    pub seq_tup_read: i64,
    /// Index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_scan: Option<i64>,
    /// Live rows fetched by index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_tup_fetch: Option<i64>,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub n_tup_ins: i64,
    /// Rows updated (including HOT).
    #[column(c, unit = count)]
    pub n_tup_upd: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub n_tup_del: i64,
    /// Rows HOT-updated.
    #[column(c, unit = count)]
    pub n_tup_hot_upd: i64,
    /// Rows updated to a new page (PG16+).
    #[column(c, unit = count)]
    pub n_tup_newpage_upd: i64,
    /// Estimated live rows.
    #[column(g, unit = count)]
    pub n_live_tup: i64,
    /// Estimated dead rows.
    #[column(g, unit = count)]
    pub n_dead_tup: i64,
    /// Rows modified since the last analyze.
    #[column(g, unit = count)]
    pub n_mod_since_analyze: i64,
    /// Rows inserted since the last vacuum (PG13+).
    #[column(g, unit = count)]
    pub n_ins_since_vacuum: i64,
    /// Manual vacuums.
    #[column(c, unit = count)]
    pub vacuum_count: i64,
    /// Autovacuums.
    #[column(c, unit = count)]
    pub autovacuum_count: i64,
    /// Manual analyzes.
    #[column(c, unit = count)]
    pub analyze_count: i64,
    /// Autoanalyzes.
    #[column(c, unit = count)]
    pub autoanalyze_count: i64,
    /// Last manual vacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_vacuum: Option<Ts>,
    /// Last autovacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autovacuum: Option<Ts>,
    /// Last manual analyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_analyze: Option<Ts>,
    /// Last autoanalyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autoanalyze: Option<Ts>,
    /// Last sequential scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_seq_scan: Option<Ts>,
    /// Last index scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_idx_scan: Option<Ts>,
    /// Cumulative manual-vacuum time in milliseconds (PG18+).
    #[column(c, unit = milliseconds)]
    pub total_vacuum_time: f64,
    /// Cumulative autovacuum time in milliseconds (PG18+).
    #[column(c, unit = milliseconds)]
    pub total_autovacuum_time: f64,
    /// Cumulative manual-analyze time in milliseconds (PG18+).
    #[column(c, unit = milliseconds)]
    pub total_analyze_time: f64,
    /// Cumulative autoanalyze time in milliseconds (PG18+).
    #[column(c, unit = milliseconds)]
    pub total_autoanalyze_time: f64,
    /// Main-fork size in bytes (`pg_relation_size`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// TOAST table + its indexes size in bytes; `None` when no TOAST relation.
    #[column(g, unit = bytes)]
    pub toast_bytes: Option<i64>,
    /// TOAST live tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_live_tup: Option<i64>,
    /// TOAST dead tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_dead_tup: Option<i64>,
    /// Last TOAST autovacuum; `None` when no TOAST relation or never.
    #[column(g, unit = microseconds)]
    pub toast_last_autovacuum: Option<Ts>,
    /// Age of `relfrozenxid` in transactions; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub xid_age: Option<i64>,
    /// Age of `relminmxid` in multixacts; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub mxid_age: Option<i64>,
    /// Planner row estimate (`pg_class.reltuples`); `-1` means never analyzed (PG14+).
    #[column(g, unit = count)]
    pub reltuples: i64,
    /// Heap block reads reported by `pg_statio_user_tables`.
    #[column(c, unit = count)]
    pub heap_blks_read: i64,
    /// Heap buffer hits.
    #[column(c, unit = count)]
    pub heap_blks_hit: i64,
    /// Index block reads; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_read: Option<i64>,
    /// Index buffer hits; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_hit: Option<i64>,
    /// TOAST block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_read: Option<i64>,
    /// TOAST buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_hit: Option<i64>,
    /// TOAST-index block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_read: Option<i64>,
    /// TOAST-index buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_hit: Option<i64>,
}

/// Type `1_013_007`: `pg_stat_user_tables` on PG 16-17 (V2 plus
/// `n_tup_newpage_upd` and the `last_seq_scan`/`last_idx_scan` timestamps).
///
/// One row per selected table per database. `idx_*` columns are `None` when the
/// table has no indexes; `toast_*` columns are `None` when it has no TOAST
/// relation; `last_*` timestamps are `None` when the event never happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_013_007,
    name = "pg_stat_user_tables",
    semantics = snapshot_full,
    sort_key("datid", "relid", "ts"),
    identity("datid", "relid")
)]
pub struct PgStatUserTablesV3 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Table oid.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Effective tablespace oid; `None` for a storage-less partitioned parent.
    #[column(l)]
    pub tablespace_oid: Option<u32>,
    /// Effective tablespace name; `None` for a storage-less parent or missing label.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Sequential scans.
    #[column(c, unit = count)]
    pub seq_scan: i64,
    /// Live rows fetched by sequential scans.
    #[column(c, unit = count)]
    pub seq_tup_read: i64,
    /// Index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_scan: Option<i64>,
    /// Live rows fetched by index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_tup_fetch: Option<i64>,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub n_tup_ins: i64,
    /// Rows updated (including HOT).
    #[column(c, unit = count)]
    pub n_tup_upd: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub n_tup_del: i64,
    /// Rows HOT-updated.
    #[column(c, unit = count)]
    pub n_tup_hot_upd: i64,
    /// Rows updated to a new page (PG16+).
    #[column(c, unit = count)]
    pub n_tup_newpage_upd: i64,
    /// Estimated live rows.
    #[column(g, unit = count)]
    pub n_live_tup: i64,
    /// Estimated dead rows.
    #[column(g, unit = count)]
    pub n_dead_tup: i64,
    /// Rows modified since the last analyze.
    #[column(g, unit = count)]
    pub n_mod_since_analyze: i64,
    /// Rows inserted since the last vacuum (PG13+).
    #[column(g, unit = count)]
    pub n_ins_since_vacuum: i64,
    /// Manual vacuums.
    #[column(c, unit = count)]
    pub vacuum_count: i64,
    /// Autovacuums.
    #[column(c, unit = count)]
    pub autovacuum_count: i64,
    /// Manual analyzes.
    #[column(c, unit = count)]
    pub analyze_count: i64,
    /// Autoanalyzes.
    #[column(c, unit = count)]
    pub autoanalyze_count: i64,
    /// Last manual vacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_vacuum: Option<Ts>,
    /// Last autovacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autovacuum: Option<Ts>,
    /// Last manual analyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_analyze: Option<Ts>,
    /// Last autoanalyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autoanalyze: Option<Ts>,
    /// Last sequential scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_seq_scan: Option<Ts>,
    /// Last index scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_idx_scan: Option<Ts>,
    /// Main-fork size in bytes (`pg_relation_size`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// TOAST table + its indexes size in bytes; `None` when no TOAST relation.
    #[column(g, unit = bytes)]
    pub toast_bytes: Option<i64>,
    /// TOAST live tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_live_tup: Option<i64>,
    /// TOAST dead tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_dead_tup: Option<i64>,
    /// Last TOAST autovacuum; `None` when no TOAST relation or never.
    #[column(g, unit = microseconds)]
    pub toast_last_autovacuum: Option<Ts>,
    /// Age of `relfrozenxid` in transactions; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub xid_age: Option<i64>,
    /// Age of `relminmxid` in multixacts; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub mxid_age: Option<i64>,
    /// Planner row estimate (`pg_class.reltuples`); `-1` means never analyzed (PG14+).
    #[column(g, unit = count)]
    pub reltuples: i64,
    /// Heap block reads reported by `pg_statio_user_tables`.
    #[column(c, unit = count)]
    pub heap_blks_read: i64,
    /// Heap buffer hits.
    #[column(c, unit = count)]
    pub heap_blks_hit: i64,
    /// Index block reads; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_read: Option<i64>,
    /// Index buffer hits; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_hit: Option<i64>,
    /// TOAST block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_read: Option<i64>,
    /// TOAST buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_hit: Option<i64>,
    /// TOAST-index block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_read: Option<i64>,
    /// TOAST-index buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_hit: Option<i64>,
}

/// Type `1_013_006`: `pg_stat_user_tables` on PG 13-15 (V1 plus
/// `n_ins_since_vacuum`, no PG16 columns). Column meanings match
/// [`PgStatUserTablesV3`] for fields present in this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_013_006,
    name = "pg_stat_user_tables",
    semantics = snapshot_full,
    sort_key("datid", "relid", "ts"),
    identity("datid", "relid")
)]
pub struct PgStatUserTablesV2 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Table oid.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Effective tablespace oid; `None` for a storage-less partitioned parent.
    #[column(l)]
    pub tablespace_oid: Option<u32>,
    /// Effective tablespace name; `None` for a storage-less parent or missing label.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Sequential scans.
    #[column(c, unit = count)]
    pub seq_scan: i64,
    /// Live rows fetched by sequential scans.
    #[column(c, unit = count)]
    pub seq_tup_read: i64,
    /// Index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_scan: Option<i64>,
    /// Live rows fetched by index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_tup_fetch: Option<i64>,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub n_tup_ins: i64,
    /// Rows updated (including HOT).
    #[column(c, unit = count)]
    pub n_tup_upd: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub n_tup_del: i64,
    /// Rows HOT-updated.
    #[column(c, unit = count)]
    pub n_tup_hot_upd: i64,
    /// Estimated live rows.
    #[column(g, unit = count)]
    pub n_live_tup: i64,
    /// Estimated dead rows.
    #[column(g, unit = count)]
    pub n_dead_tup: i64,
    /// Rows modified since the last analyze.
    #[column(g, unit = count)]
    pub n_mod_since_analyze: i64,
    /// Rows inserted since the last vacuum (PG13+).
    #[column(g, unit = count)]
    pub n_ins_since_vacuum: i64,
    /// Manual vacuums.
    #[column(c, unit = count)]
    pub vacuum_count: i64,
    /// Autovacuums.
    #[column(c, unit = count)]
    pub autovacuum_count: i64,
    /// Manual analyzes.
    #[column(c, unit = count)]
    pub analyze_count: i64,
    /// Autoanalyzes.
    #[column(c, unit = count)]
    pub autoanalyze_count: i64,
    /// Last manual vacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_vacuum: Option<Ts>,
    /// Last autovacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autovacuum: Option<Ts>,
    /// Last manual analyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_analyze: Option<Ts>,
    /// Last autoanalyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autoanalyze: Option<Ts>,
    /// Main-fork size in bytes (`pg_relation_size`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// TOAST table + its indexes size in bytes; `None` when no TOAST relation.
    #[column(g, unit = bytes)]
    pub toast_bytes: Option<i64>,
    /// TOAST live tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_live_tup: Option<i64>,
    /// TOAST dead tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_dead_tup: Option<i64>,
    /// Last TOAST autovacuum; `None` when no TOAST relation or never.
    #[column(g, unit = microseconds)]
    pub toast_last_autovacuum: Option<Ts>,
    /// Age of `relfrozenxid` in transactions; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub xid_age: Option<i64>,
    /// Age of `relminmxid` in multixacts; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub mxid_age: Option<i64>,
    /// Planner row estimate (`pg_class.reltuples`); `-1` means never analyzed (PG14+).
    #[column(g, unit = count)]
    pub reltuples: i64,
    /// Heap block reads reported by `pg_statio_user_tables`.
    #[column(c, unit = count)]
    pub heap_blks_read: i64,
    /// Heap buffer hits.
    #[column(c, unit = count)]
    pub heap_blks_hit: i64,
    /// Index block reads; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_read: Option<i64>,
    /// Index buffer hits; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_hit: Option<i64>,
    /// TOAST block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_read: Option<i64>,
    /// TOAST buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_hit: Option<i64>,
    /// TOAST-index block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_read: Option<i64>,
    /// TOAST-index buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_hit: Option<i64>,
}

/// Type `1_013_005`: `pg_stat_user_tables` on PG 10-12 (base layout, no
/// `n_ins_since_vacuum` and no PG16 columns). Column meanings match
/// [`PgStatUserTablesV3`] for fields present in this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_013_005,
    name = "pg_stat_user_tables",
    semantics = snapshot_full,
    sort_key("datid", "relid", "ts"),
    identity("datid", "relid")
)]
pub struct PgStatUserTablesV1 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Table oid.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Effective tablespace oid; `None` for a storage-less partitioned parent.
    #[column(l)]
    pub tablespace_oid: Option<u32>,
    /// Effective tablespace name; `None` for a storage-less parent or missing label.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Sequential scans.
    #[column(c, unit = count)]
    pub seq_scan: i64,
    /// Live rows fetched by sequential scans.
    #[column(c, unit = count)]
    pub seq_tup_read: i64,
    /// Index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_scan: Option<i64>,
    /// Live rows fetched by index scans; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_tup_fetch: Option<i64>,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub n_tup_ins: i64,
    /// Rows updated (including HOT).
    #[column(c, unit = count)]
    pub n_tup_upd: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub n_tup_del: i64,
    /// Rows HOT-updated.
    #[column(c, unit = count)]
    pub n_tup_hot_upd: i64,
    /// Estimated live rows.
    #[column(g, unit = count)]
    pub n_live_tup: i64,
    /// Estimated dead rows.
    #[column(g, unit = count)]
    pub n_dead_tup: i64,
    /// Rows modified since the last analyze.
    #[column(g, unit = count)]
    pub n_mod_since_analyze: i64,
    /// Manual vacuums.
    #[column(c, unit = count)]
    pub vacuum_count: i64,
    /// Autovacuums.
    #[column(c, unit = count)]
    pub autovacuum_count: i64,
    /// Manual analyzes.
    #[column(c, unit = count)]
    pub analyze_count: i64,
    /// Autoanalyzes.
    #[column(c, unit = count)]
    pub autoanalyze_count: i64,
    /// Last manual vacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_vacuum: Option<Ts>,
    /// Last autovacuum; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autovacuum: Option<Ts>,
    /// Last manual analyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_analyze: Option<Ts>,
    /// Last autoanalyze; `None` if never.
    #[column(g, unit = microseconds)]
    pub last_autoanalyze: Option<Ts>,
    /// Main-fork size in bytes (`pg_relation_size`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// TOAST table + its indexes size in bytes; `None` when no TOAST relation.
    #[column(g, unit = bytes)]
    pub toast_bytes: Option<i64>,
    /// TOAST live tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_live_tup: Option<i64>,
    /// TOAST dead tuples; `None` when no TOAST relation.
    #[column(g, unit = count)]
    pub toast_n_dead_tup: Option<i64>,
    /// Last TOAST autovacuum; `None` when no TOAST relation or never.
    #[column(g, unit = microseconds)]
    pub toast_last_autovacuum: Option<Ts>,
    /// Age of `relfrozenxid` in transactions; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub xid_age: Option<i64>,
    /// Age of `relminmxid` in multixacts; `None` for partitioned parents.
    #[column(g, unit = count)]
    pub mxid_age: Option<i64>,
    /// Planner row estimate (`pg_class.reltuples`); `-1` means never analyzed (PG14+).
    #[column(g, unit = count)]
    pub reltuples: i64,
    /// Heap block reads reported by `pg_statio_user_tables`.
    #[column(c, unit = count)]
    pub heap_blks_read: i64,
    /// Heap buffer hits.
    #[column(c, unit = count)]
    pub heap_blks_hit: i64,
    /// Index block reads; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_read: Option<i64>,
    /// Index buffer hits; `None` when the table has no indexes.
    #[column(c, unit = count)]
    pub idx_blks_hit: Option<i64>,
    /// TOAST block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_read: Option<i64>,
    /// TOAST buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub toast_blks_hit: Option<i64>,
    /// TOAST-index block reads; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_read: Option<i64>,
    /// TOAST-index buffer hits; `None` when no TOAST relation.
    #[column(c, unit = count)]
    pub tidx_blks_hit: Option<i64>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_user_tables.rs"]
mod tests;
