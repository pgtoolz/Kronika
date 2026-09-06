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
mod tests {
    use super::{PgStatUserTablesV1, PgStatUserTablesV2, PgStatUserTablesV3, PgStatUserTablesV4};
    use crate::{Section, StrId, Ts, Unit, VerifiedSection};

    fn assert_common_contract(c: crate::TypeContract) {
        assert_eq!(
            c.column("xid_age").map(|column| column.nullable),
            Some(true)
        );
        assert_eq!(
            c.column("mxid_age").map(|column| column.nullable),
            Some(true)
        );
        for name in [
            "last_vacuum",
            "last_autovacuum",
            "last_analyze",
            "last_autoanalyze",
            "toast_last_autovacuum",
        ] {
            assert_eq!(
                c.column(name).and_then(|column| column.unit),
                Some(Unit::Microseconds)
            );
        }
        for name in ["last_seq_scan", "last_idx_scan"] {
            if let Some(column) = c.column(name) {
                assert_eq!(column.unit, Some(Unit::Microseconds));
            }
        }
    }

    fn v4_row(ts: i64, datid: u32, relid: u32) -> PgStatUserTablesV4 {
        PgStatUserTablesV4 {
            ts: Ts(ts),
            datid,
            datname: StrId(u64::from(datid) | 1),
            relid,
            schemaname: StrId(2),
            relname: StrId(u64::from(relid) | 1),
            tablespace_oid: Some(1_663),
            tablespace: Some(StrId(4)),
            seq_scan: 10,
            seq_tup_read: 1_000,
            idx_scan: None,
            idx_tup_fetch: None,
            n_tup_ins: 50,
            n_tup_upd: 30,
            n_tup_del: 10,
            n_tup_hot_upd: 5,
            n_tup_newpage_upd: 0,
            n_live_tup: 900,
            n_dead_tup: 40,
            n_mod_since_analyze: 70,
            n_ins_since_vacuum: 20,
            vacuum_count: 1,
            autovacuum_count: 3,
            analyze_count: 1,
            autoanalyze_count: 2,
            last_vacuum: Some(Ts(ts - 10)),
            last_autovacuum: None,
            last_analyze: None,
            last_autoanalyze: Some(Ts(ts - 5)),
            last_seq_scan: Some(Ts(ts - 1)),
            last_idx_scan: None,
            total_vacuum_time: 12.5,
            total_autovacuum_time: 340.0,
            total_analyze_time: 7.5,
            total_autoanalyze_time: 21.0,
            main_fork_bytes: 8_192,
            toast_bytes: None,
            toast_n_live_tup: None,
            toast_n_dead_tup: None,
            toast_last_autovacuum: None,
            xid_age: Some(100_000_000),
            mxid_age: Some(5_000_000),
            reltuples: 900,
            heap_blks_read: 400,
            heap_blks_hit: 90_000,
            idx_blks_read: None,
            idx_blks_hit: None,
            toast_blks_read: None,
            toast_blks_hit: None,
            tidx_blks_read: None,
            tidx_blks_hit: None,
        }
    }

    #[test]
    fn v4_contract_shape() {
        let c = PgStatUserTablesV4::CONTRACT;
        assert_eq!(c.type_id.get(), 1_013_008);
        assert_eq!(c.columns.len(), 51);
        assert_eq!(c.sort_key, ["datid", "relid", "ts"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("relid").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("tablespace_oid").map(|col| col.nullable),
            Some(true)
        );
        assert_eq!(c.column("tablespace").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(true));
        assert!(c.column("total_vacuum_time").is_some());
        assert!(c.column("total_autovacuum_time").is_some());
        assert!(c.column("total_analyze_time").is_some());
        assert!(c.column("total_autoanalyze_time").is_some());
        assert!(c.column("main_fork_bytes").is_some());
        assert!(c.column("size_bytes").is_none());
        assert_common_contract(c);
    }

    #[test]
    fn v4_roundtrip() {
        let mut null_ages = v4_row(5, 5, 16_384);
        null_ages.xid_age = None;
        null_ages.mxid_age = None;
        crate::assert_roundtrips(&[
            null_ages,
            v4_row(1_000, 5, 16_384),
            v4_row(1_000, 5, 16_385),
        ]);
    }

    #[test]
    fn v3_contract_shape() {
        let c = PgStatUserTablesV3::CONTRACT;
        assert_eq!(c.type_id.get(), 1_013_007);
        assert_eq!(c.columns.len(), 47);
        assert_eq!(c.sort_key, ["datid", "relid", "ts"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("relid").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("datname").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("toast_bytes").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("last_vacuum").map(|col| col.nullable), Some(true));
        assert!(c.column("n_tup_newpage_upd").is_some());
        assert!(c.column("last_seq_scan").is_some());
        assert_common_contract(c);
    }

    #[test]
    fn v2_drops_pg16_columns() {
        let c = PgStatUserTablesV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_013_006);
        assert_eq!(c.columns.len(), 44);
        assert!(c.column("n_ins_since_vacuum").is_some());
        assert!(c.column("n_tup_newpage_upd").is_none());
        assert!(c.column("last_seq_scan").is_none());
        assert_common_contract(c);
    }

    #[test]
    fn v1_is_base_layout() {
        let c = PgStatUserTablesV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_013_005);
        assert_eq!(c.columns.len(), 43);
        assert!(c.column("n_ins_since_vacuum").is_none());
        assert_common_contract(c);
    }

    fn v3_row(ts: i64, datid: u32, relid: u32) -> PgStatUserTablesV3 {
        PgStatUserTablesV3 {
            ts: Ts(ts),
            datid,
            datname: StrId(u64::from(datid) | 1),
            relid,
            schemaname: StrId(2),
            relname: StrId(u64::from(relid) | 1),
            tablespace_oid: Some(1_663),
            tablespace: Some(StrId(4)),
            seq_scan: 10,
            seq_tup_read: 1_000,
            idx_scan: None,
            idx_tup_fetch: None,
            n_tup_ins: 50,
            n_tup_upd: 30,
            n_tup_del: 10,
            n_tup_hot_upd: 5,
            n_tup_newpage_upd: 0,
            n_live_tup: 900,
            n_dead_tup: 40,
            n_mod_since_analyze: 70,
            n_ins_since_vacuum: 20,
            vacuum_count: 1,
            autovacuum_count: 3,
            analyze_count: 1,
            autoanalyze_count: 2,
            last_vacuum: Some(Ts(ts - 10)),
            last_autovacuum: None,
            last_analyze: None,
            last_autoanalyze: Some(Ts(ts - 5)),
            last_seq_scan: Some(Ts(ts - 1)),
            last_idx_scan: None,
            main_fork_bytes: 8_192,
            toast_bytes: None,
            toast_n_live_tup: None,
            toast_n_dead_tup: None,
            toast_last_autovacuum: None,
            xid_age: Some(100_000_000),
            mxid_age: Some(5_000_000),
            reltuples: 900,
            heap_blks_read: 400,
            heap_blks_hit: 90_000,
            idx_blks_read: None,
            idx_blks_hit: None,
            toast_blks_read: None,
            toast_blks_hit: None,
            tidx_blks_read: None,
            tidx_blks_hit: None,
        }
    }

    #[test]
    fn v3_roundtrip() {
        crate::assert_roundtrips(&[v3_row(1_000, 5, 16_384), v3_row(1_000, 5, 16_385)]);
    }

    #[test]
    fn v3_encode_sorts_by_datid_relid_ts() {
        let bytes = PgStatUserTablesV3::encode(&[
            v3_row(1_000, 9, 16_385),
            v3_row(1_000, 1, 16_390),
            v3_row(1_000, 1, 16_384),
        ])
        .expect("encode");
        let decoded =
            PgStatUserTablesV3::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded
                .iter()
                .map(|r| (r.datid, r.relid))
                .collect::<Vec<_>>(),
            [(1, 16_384), (1, 16_390), (9, 16_385)]
        );
    }
}
