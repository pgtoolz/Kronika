//! Type `1_002_001`..`1_002_006`: `pg_stat_statements`.
//!
//! Per-statement execution counters, one row per `(userid, dbid, queryid)` and,
//! from extension 1.9, also per `toplevel`. The `dbid` distinguishes databases,
//! so one query from a database with the extension installed reads the shared
//! instance-wide rows. The layout is selected by the *extension* version, not the
//! server major, because the extension can be pinned independently of the server.
//!
//! The extension column set changes across releases:
//! - 1.8 (PG13) renamed `total_time`/`min_time`/`max_time`/`mean_time`/
//!   `stddev_time` to their `*_exec_time` forms and added the planning columns
//!   (`plans`, `total_plan_time`, ...) and `wal_records`/`wal_fpi`/`wal_bytes`;
//! - 1.9 (PG14) added `toplevel`;
//! - 1.10 (PG15) added `temp_blk_read_time`/`temp_blk_write_time` and the eight
//!   JIT columns;
//! - 1.11 (PG17) renamed `blk_read_time`/`blk_write_time` to
//!   `shared_blk_*_time`, added the `local_blk_*_time` pair, `jit_deform_count`/
//!   `jit_deform_time`, and the `stats_since`/`minmax_stats_since` timestamps;
//! - 1.12 (PG18) added `wal_buffers_full` and the parallel-worker counters.
//!
//! `queryid` and `query` are nullable in the format. The collector omits rows
//! whose `queryid` is privilege-masked and bounds query text in SQL. Timing
//! columns are `f64`, so the layouts derive `PartialEq` but not `Eq`.

use crate::{Section, StrId, Ts};

/// Type `1_002_006`: `pg_stat_statements` on extension 1.12 (PG18).
///
/// One row per `(userid, dbid, queryid, toplevel)`. Adds `wal_buffers_full` and
/// the parallel-worker counters over [`PgStatStatementsV5`]. Query text is
/// optional in the format; the collector bounds it to 65,536 characters in
/// SQL. The planning and JIT columns are `0` when `track_planning` or JIT is
/// off, not `NULL`.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_006,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "toplevel", "ts"),
    identity("queryid", "userid", "dbid", "toplevel")
)]
pub struct PgStatStatementsV6 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Whether the statement ran at the top level (not nested); extension 1.9+.
    #[column(l)]
    pub toplevel: bool,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Times the statement was planned; `0` without `track_planning`.
    #[column(c, unit = count)]
    pub plans: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_exec_time: f64,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_exec_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_exec_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_exec_time: f64,
    /// Population standard deviation of execution time, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_exec_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
    /// Population standard deviation of planning time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_plan_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub local_blk_read_time: f64,
    /// Time writing local blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub local_blk_write_time: f64,
    /// Time reading temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_write_time: f64,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated.
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
    /// Times a WAL write waited on a full WAL buffer (extension 1.12+).
    #[column(c, unit = count)]
    pub wal_buffers_full: i64,
    /// JIT-compiled functions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_functions: i64,
    /// Time spent generating JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_generation_time: f64,
    /// JIT inlining passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_inlining_count: i64,
    /// Time spent inlining JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_inlining_time: f64,
    /// JIT optimization passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_optimization_count: i64,
    /// Time spent optimizing JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_optimization_time: f64,
    /// JIT code emissions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_emission_count: i64,
    /// Time spent emitting JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_emission_time: f64,
    /// JIT tuple-deforming passes (extension 1.11+); `0` without JIT.
    #[column(c, unit = count)]
    pub jit_deform_count: i64,
    /// Time spent deforming tuples in JIT code, milliseconds (extension 1.11+).
    #[column(c, unit = milliseconds)]
    pub jit_deform_time: f64,
    /// Parallel workers planned for launch (extension 1.12+).
    #[column(c, unit = count)]
    pub parallel_workers_to_launch: i64,
    /// Parallel workers actually launched (extension 1.12+).
    #[column(c, unit = count)]
    pub parallel_workers_launched: i64,
    /// Time the statistics for this row began accumulating; extension 1.11+.
    #[column(g, unit = microseconds)]
    pub stats_since: Ts,
    /// Time the min/max statistics for this row were last reset; extension 1.11+.
    #[column(g, unit = microseconds)]
    pub minmax_stats_since: Ts,
}

/// Type `1_002_005`: `pg_stat_statements` on extension 1.11 (PG17).
///
/// [`PgStatStatementsV6`] without `wal_buffers_full` and the parallel-worker
/// counters. Column meanings match [`PgStatStatementsV6`] for shared fields.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_005,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "toplevel", "ts"),
    identity("queryid", "userid", "dbid", "toplevel")
)]
pub struct PgStatStatementsV5 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Whether the statement ran at the top level (not nested).
    #[column(l)]
    pub toplevel: bool,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Times the statement was planned; `0` without `track_planning`.
    #[column(c, unit = count)]
    pub plans: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_exec_time: f64,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_exec_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_exec_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_exec_time: f64,
    /// Population standard deviation of execution time, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_exec_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
    /// Population standard deviation of planning time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_plan_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub local_blk_read_time: f64,
    /// Time writing local blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub local_blk_write_time: f64,
    /// Time reading temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_write_time: f64,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated.
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
    /// JIT-compiled functions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_functions: i64,
    /// Time spent generating JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_generation_time: f64,
    /// JIT inlining passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_inlining_count: i64,
    /// Time spent inlining JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_inlining_time: f64,
    /// JIT optimization passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_optimization_count: i64,
    /// Time spent optimizing JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_optimization_time: f64,
    /// JIT code emissions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_emission_count: i64,
    /// Time spent emitting JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_emission_time: f64,
    /// JIT tuple-deforming passes (extension 1.11+); `0` without JIT.
    #[column(c, unit = count)]
    pub jit_deform_count: i64,
    /// Time spent deforming tuples in JIT code, milliseconds (extension 1.11+).
    #[column(c, unit = milliseconds)]
    pub jit_deform_time: f64,
    /// Time the statistics for this row began accumulating.
    #[column(g, unit = microseconds)]
    pub stats_since: Ts,
    /// Time the min/max statistics for this row were last reset.
    #[column(g, unit = microseconds)]
    pub minmax_stats_since: Ts,
}

/// Type `1_002_004`: `pg_stat_statements` on extension 1.10 (PG15-16).
///
/// [`PgStatStatementsV5`] with the pre-1.11 block-timing names
/// (`blk_read_time`/`blk_write_time`) and without the `local_blk_*_time` pair,
/// `jit_deform_*`, and the `stats_since`/`minmax_stats_since` timestamps. Column
/// meanings match [`PgStatStatementsV6`] for shared fields.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_004,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "toplevel", "ts"),
    identity("queryid", "userid", "dbid", "toplevel")
)]
pub struct PgStatStatementsV4 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Whether the statement ran at the top level (not nested).
    #[column(l)]
    pub toplevel: bool,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Times the statement was planned; `0` without `track_planning`.
    #[column(c, unit = count)]
    pub plans: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_exec_time: f64,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_exec_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_exec_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_exec_time: f64,
    /// Population standard deviation of execution time, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_exec_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
    /// Population standard deviation of planning time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_plan_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time writing shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// Time reading temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub temp_blk_write_time: f64,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated.
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
    /// JIT-compiled functions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_functions: i64,
    /// Time spent generating JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_generation_time: f64,
    /// JIT inlining passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_inlining_count: i64,
    /// Time spent inlining JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_inlining_time: f64,
    /// JIT optimization passes; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_optimization_count: i64,
    /// Time spent optimizing JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_optimization_time: f64,
    /// JIT code emissions; `0` without JIT.
    #[column(c, unit = count)]
    pub jit_emission_count: i64,
    /// Time spent emitting JIT code, milliseconds; `0` without JIT.
    #[column(c, unit = milliseconds)]
    pub jit_emission_time: f64,
}

/// Type `1_002_003`: `pg_stat_statements` on extension 1.9 (PG14).
///
/// [`PgStatStatementsV4`] without the `temp_blk_*_time` pair and the eight JIT
/// columns. Column meanings match [`PgStatStatementsV6`] for shared fields.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_003,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "toplevel", "ts"),
    identity("queryid", "userid", "dbid", "toplevel")
)]
pub struct PgStatStatementsV3 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Whether the statement ran at the top level (not nested).
    #[column(l)]
    pub toplevel: bool,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Times the statement was planned; `0` without `track_planning`.
    #[column(c, unit = count)]
    pub plans: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_exec_time: f64,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_exec_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_exec_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_exec_time: f64,
    /// Population standard deviation of execution time, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_exec_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
    /// Population standard deviation of planning time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_plan_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time writing shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated.
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
}

/// Type `1_002_002`: `pg_stat_statements` on extension 1.8 (PG13).
///
/// [`PgStatStatementsV3`] without `toplevel`. Column meanings match
/// [`PgStatStatementsV6`] for shared fields.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_002,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "ts"),
    identity("queryid", "userid", "dbid")
)]
pub struct PgStatStatementsV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Times the statement was planned; `0` without `track_planning`.
    #[column(c, unit = count)]
    pub plans: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_exec_time: f64,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_exec_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_exec_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_exec_time: f64,
    /// Population standard deviation of execution time, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_exec_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
    /// Population standard deviation of planning time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_plan_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time writing shared blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// WAL records generated.
    #[column(c, unit = count)]
    pub wal_records: i64,
    /// WAL full-page images generated.
    #[column(c, unit = count)]
    pub wal_fpi: i64,
    /// WAL bytes generated.
    #[column(c, unit = bytes)]
    pub wal_bytes: i64,
}

/// Type `1_002_001`: `pg_stat_statements` on extension 1.5-1.7 (PG10-12).
///
/// The legacy layout: the timing columns keep their unqualified names
/// (`total_time`/`min_time`/`max_time`/`mean_time`/`stddev_time`), and there are
/// no planning, WAL, JIT, or `toplevel` columns. Column meanings otherwise match
/// [`PgStatStatementsV6`] for shared fields.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_002_001,
    name = "pg_stat_statements",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "ts"),
    identity("queryid", "userid", "dbid")
)]
pub struct PgStatStatementsV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Query id; `None` when unavailable or privilege-masked.
    #[column(l)]
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statement ran in.
    #[column(l)]
    pub dbid: u32,
    /// Database name resolved from `dbid`; `None` when `dbid` has no `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Statement text; the collector bounds present text to 65,536 characters.
    #[column(l)]
    pub query: Option<StrId>,
    /// Times the statement was executed.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
    /// Total time spent in the statement, milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_time: f64,
    /// Minimum time spent in the statement, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_time: f64,
    /// Maximum time spent in the statement, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_time: f64,
    /// Mean time spent in the statement, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_time: f64,
    /// Population standard deviation of time spent, milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub stddev_time: f64,
    /// Shared-block buffer hits.
    #[column(c, unit = count)]
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    #[column(c, unit = count)]
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    #[column(c, unit = count)]
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    #[column(c, unit = count)]
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    #[column(c, unit = count)]
    pub local_blks_hit: i64,
    /// Local blocks read.
    #[column(c, unit = count)]
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    #[column(c, unit = count)]
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    #[column(c, unit = count)]
    pub local_blks_written: i64,
    /// Temp blocks read.
    #[column(c, unit = count)]
    pub temp_blks_read: i64,
    /// Temp blocks written.
    #[column(c, unit = count)]
    pub temp_blks_written: i64,
    /// Time reading blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time writing blocks, milliseconds; `0` without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
}

#[cfg(test)]
mod tests {
    use super::{
        PgStatStatementsV1, PgStatStatementsV2, PgStatStatementsV3, PgStatStatementsV4,
        PgStatStatementsV5, PgStatStatementsV6,
    };
    use crate::{Section, StrId, Ts, Unit, VerifiedSection};

    fn assert_shared_block_unit(c: crate::TypeContract) {
        assert_eq!(
            c.column("shared_blks_read").and_then(|column| column.unit),
            Some(Unit::Count)
        );
    }

    fn assert_stats_timestamp_units(c: crate::TypeContract) {
        for name in ["stats_since", "minmax_stats_since"] {
            let column = c.column(name).expect("statistics timestamp");
            assert_eq!(column.unit, Some(Unit::Microseconds));
            assert!(!column.nullable);
        }
    }

    fn v6_row(ts: i64, dbid: u32, userid: u32, queryid: Option<i64>) -> PgStatStatementsV6 {
        PgStatStatementsV6 {
            ts: Ts(ts),
            queryid,
            userid,
            dbid,
            toplevel: true,
            datname: Some(StrId(u64::from(dbid) | 1)),
            usename: Some(StrId(u64::from(userid) | 1)),
            query: queryid.map(|q| StrId(q.cast_unsigned() | 1)),
            calls: 100,
            rows: 5_000,
            plans: 90,
            total_exec_time: 1_234.5,
            total_plan_time: 12.5,
            min_exec_time: 0.5,
            max_exec_time: 40.0,
            mean_exec_time: 12.3,
            stddev_exec_time: 3.1,
            min_plan_time: 0.1,
            max_plan_time: 1.0,
            mean_plan_time: 0.2,
            stddev_plan_time: 0.05,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            shared_blk_read_time: 12.5,
            shared_blk_write_time: 3.0,
            local_blk_read_time: 0.0,
            local_blk_write_time: 0.0,
            temp_blk_read_time: 0.0,
            temp_blk_write_time: 0.0,
            wal_records: 42,
            wal_fpi: 3,
            wal_bytes: 8_192,
            wal_buffers_full: 1,
            jit_functions: 0,
            jit_generation_time: 0.0,
            jit_inlining_count: 0,
            jit_inlining_time: 0.0,
            jit_optimization_count: 0,
            jit_optimization_time: 0.0,
            jit_emission_count: 0,
            jit_emission_time: 0.0,
            jit_deform_count: 0,
            jit_deform_time: 0.0,
            parallel_workers_to_launch: 4,
            parallel_workers_launched: 3,
            stats_since: Ts(ts - 100),
            minmax_stats_since: Ts(ts - 50),
        }
    }

    #[test]
    fn v6_contract_shape() {
        let c = PgStatStatementsV6::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_006);
        assert_eq!(c.columns.len(), 55);
        assert_eq!(c.sort_key, ["dbid", "userid", "queryid", "toplevel", "ts"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("dbid").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("userid").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("queryid").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("query").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("datname").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("toplevel").map(|col| col.nullable), Some(false));
        assert!(c.column("wal_buffers_full").is_some());
        assert!(c.column("parallel_workers_launched").is_some());
        assert!(c.column("shared_blk_read_time").is_some());
        assert!(c.column("local_blk_read_time").is_some());
        assert!(c.column("jit_deform_time").is_some());
        assert_eq!(c.column("stats_since").map(|col| col.nullable), Some(false));
        assert_shared_block_unit(c);
        assert_stats_timestamp_units(c);
        // No legacy names on the newest layout.
        assert!(c.column("total_time").is_none());
        assert!(c.column("blk_read_time").is_none());
    }

    #[test]
    fn v6_roundtrip_and_null_preservation() {
        // A row with a resolved queryid/query and one with nullable identity
        // and text, as produced under restricted visibility.
        crate::assert_roundtrips(&[v6_row(1_000, 5, 10, Some(777)), v6_row(1_000, 5, 11, None)]);
    }

    #[test]
    fn v6_encode_sorts_by_dbid_then_userid_then_ts() {
        let bytes = PgStatStatementsV6::encode(&[
            v6_row(1_000, 9, 3, Some(1)),
            v6_row(1_000, 1, 8, Some(2)),
            v6_row(1_000, 1, 2, Some(3)),
        ])
        .expect("encode");
        let decoded =
            PgStatStatementsV6::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded
                .iter()
                .map(|r| (r.dbid, r.userid))
                .collect::<Vec<_>>(),
            [(1, 2), (1, 8), (9, 3)]
        );
    }

    fn v5_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV5 {
        PgStatStatementsV5 {
            ts: Ts(ts),
            queryid: Some(777),
            userid,
            dbid,
            toplevel: true,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            query: Some(StrId(3)),
            calls: 100,
            rows: 5_000,
            plans: 90,
            total_exec_time: 1_234.5,
            total_plan_time: 12.5,
            min_exec_time: 0.5,
            max_exec_time: 40.0,
            mean_exec_time: 12.3,
            stddev_exec_time: 3.1,
            min_plan_time: 0.1,
            max_plan_time: 1.0,
            mean_plan_time: 0.2,
            stddev_plan_time: 0.05,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            shared_blk_read_time: 12.5,
            shared_blk_write_time: 3.0,
            local_blk_read_time: 0.0,
            local_blk_write_time: 0.0,
            temp_blk_read_time: 0.0,
            temp_blk_write_time: 0.0,
            wal_records: 42,
            wal_fpi: 3,
            wal_bytes: 8_192,
            jit_functions: 0,
            jit_generation_time: 0.0,
            jit_inlining_count: 0,
            jit_inlining_time: 0.0,
            jit_optimization_count: 0,
            jit_optimization_time: 0.0,
            jit_emission_count: 0,
            jit_emission_time: 0.0,
            jit_deform_count: 0,
            jit_deform_time: 0.0,
            stats_since: Ts(ts - 100),
            minmax_stats_since: Ts(ts - 50),
        }
    }

    #[test]
    fn v5_contract_shape() {
        let c = PgStatStatementsV5::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_005);
        assert_eq!(c.columns.len(), 52);
        assert!(c.column("shared_blk_read_time").is_some());
        assert!(c.column("local_blk_write_time").is_some());
        assert!(c.column("jit_deform_count").is_some());
        assert_eq!(c.column("stats_since").map(|col| col.nullable), Some(false));
        // 1.11 renamed away the unqualified block-timing names and has no 1.12
        // columns.
        assert!(c.column("blk_read_time").is_none());
        assert!(c.column("wal_buffers_full").is_none());
        assert!(c.column("parallel_workers_launched").is_none());
        assert_shared_block_unit(c);
        assert_stats_timestamp_units(c);
    }

    #[test]
    fn v5_roundtrip() {
        crate::assert_roundtrips(&[v5_row(1_000, 5, 10), v5_row(1_000, 5, 11)]);
    }

    fn v4_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV4 {
        PgStatStatementsV4 {
            ts: Ts(ts),
            queryid: Some(777),
            userid,
            dbid,
            toplevel: true,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            query: Some(StrId(3)),
            calls: 100,
            rows: 5_000,
            plans: 90,
            total_exec_time: 1_234.5,
            total_plan_time: 12.5,
            min_exec_time: 0.5,
            max_exec_time: 40.0,
            mean_exec_time: 12.3,
            stddev_exec_time: 3.1,
            min_plan_time: 0.1,
            max_plan_time: 1.0,
            mean_plan_time: 0.2,
            stddev_plan_time: 0.05,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            blk_read_time: 12.5,
            blk_write_time: 3.0,
            temp_blk_read_time: 0.0,
            temp_blk_write_time: 0.0,
            wal_records: 42,
            wal_fpi: 3,
            wal_bytes: 8_192,
            jit_functions: 0,
            jit_generation_time: 0.0,
            jit_inlining_count: 0,
            jit_inlining_time: 0.0,
            jit_optimization_count: 0,
            jit_optimization_time: 0.0,
            jit_emission_count: 0,
            jit_emission_time: 0.0,
        }
    }

    #[test]
    fn v4_contract_shape() {
        let c = PgStatStatementsV4::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_004);
        assert_eq!(c.columns.len(), 46);
        // 1.10 uses the unqualified block-timing names and has temp timing + JIT.
        assert!(c.column("blk_read_time").is_some());
        assert!(c.column("temp_blk_read_time").is_some());
        assert!(c.column("jit_emission_time").is_some());
        // No 1.11 columns.
        assert!(c.column("shared_blk_read_time").is_none());
        assert!(c.column("local_blk_read_time").is_none());
        assert!(c.column("jit_deform_count").is_none());
        assert!(c.column("stats_since").is_none());
        assert_shared_block_unit(c);
    }

    #[test]
    fn v4_roundtrip() {
        crate::assert_roundtrips(&[v4_row(1_000, 5, 10), v4_row(1_000, 5, 11)]);
    }

    fn v3_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV3 {
        PgStatStatementsV3 {
            ts: Ts(ts),
            queryid: Some(777),
            userid,
            dbid,
            toplevel: true,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            query: Some(StrId(3)),
            calls: 100,
            rows: 5_000,
            plans: 90,
            total_exec_time: 1_234.5,
            total_plan_time: 12.5,
            min_exec_time: 0.5,
            max_exec_time: 40.0,
            mean_exec_time: 12.3,
            stddev_exec_time: 3.1,
            min_plan_time: 0.1,
            max_plan_time: 1.0,
            mean_plan_time: 0.2,
            stddev_plan_time: 0.05,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            blk_read_time: 12.5,
            blk_write_time: 3.0,
            wal_records: 42,
            wal_fpi: 3,
            wal_bytes: 8_192,
        }
    }

    #[test]
    fn v3_contract_shape() {
        let c = PgStatStatementsV3::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_003);
        assert_eq!(c.columns.len(), 36);
        // 1.9 adds toplevel but not temp timing or JIT.
        assert!(c.column("toplevel").is_some());
        assert!(c.column("wal_bytes").is_some());
        assert!(c.column("temp_blk_read_time").is_none());
        assert!(c.column("jit_functions").is_none());
        assert_shared_block_unit(c);
    }

    #[test]
    fn v3_roundtrip() {
        crate::assert_roundtrips(&[v3_row(1_000, 5, 10), v3_row(1_000, 5, 11)]);
    }

    fn v2_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV2 {
        PgStatStatementsV2 {
            ts: Ts(ts),
            queryid: Some(777),
            userid,
            dbid,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            query: Some(StrId(3)),
            calls: 100,
            rows: 5_000,
            plans: 90,
            total_exec_time: 1_234.5,
            total_plan_time: 12.5,
            min_exec_time: 0.5,
            max_exec_time: 40.0,
            mean_exec_time: 12.3,
            stddev_exec_time: 3.1,
            min_plan_time: 0.1,
            max_plan_time: 1.0,
            mean_plan_time: 0.2,
            stddev_plan_time: 0.05,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            blk_read_time: 12.5,
            blk_write_time: 3.0,
            wal_records: 42,
            wal_fpi: 3,
            wal_bytes: 8_192,
        }
    }

    #[test]
    fn v2_contract_shape() {
        let c = PgStatStatementsV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_002);
        assert_eq!(c.columns.len(), 35);
        // 1.8 has the exec/plan split and WAL, but no toplevel.
        assert!(c.column("total_exec_time").is_some());
        assert!(c.column("total_plan_time").is_some());
        assert!(c.column("wal_records").is_some());
        assert!(c.column("toplevel").is_none());
        assert!(c.column("temp_blk_read_time").is_none());
        assert_shared_block_unit(c);
    }

    #[test]
    fn v2_roundtrip() {
        crate::assert_roundtrips(&[v2_row(1_000, 5, 10), v2_row(1_000, 5, 11)]);
    }

    fn v1_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV1 {
        PgStatStatementsV1 {
            ts: Ts(ts),
            queryid: Some(777),
            userid,
            dbid,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            query: Some(StrId(3)),
            calls: 100,
            rows: 5_000,
            total_time: 1_234.5,
            min_time: 0.5,
            max_time: 40.0,
            mean_time: 12.3,
            stddev_time: 3.1,
            shared_blks_hit: 90_000,
            shared_blks_read: 4_000,
            shared_blks_dirtied: 50,
            shared_blks_written: 30,
            local_blks_hit: 0,
            local_blks_read: 0,
            local_blks_dirtied: 0,
            local_blks_written: 0,
            temp_blks_read: 0,
            temp_blks_written: 0,
            blk_read_time: 12.5,
            blk_write_time: 3.0,
        }
    }

    #[test]
    fn v1_contract_shape() {
        let c = PgStatStatementsV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_002_001);
        assert_eq!(c.columns.len(), 26);
        // The legacy layout keeps the unqualified timing names and has no
        // exec/plan split, no WAL, no toplevel.
        assert!(c.column("total_time").is_some());
        assert!(c.column("mean_time").is_some());
        assert!(c.column("total_exec_time").is_none());
        assert!(c.column("total_plan_time").is_none());
        assert!(c.column("wal_records").is_none());
        assert!(c.column("toplevel").is_none());
        assert_shared_block_unit(c);
    }

    #[test]
    fn v1_roundtrip_and_null_query() {
        let mut null_query = v1_row(5, 5, 10);
        null_query.query = None;
        null_query.queryid = None;
        crate::assert_roundtrips(&[null_query, v1_row(1_000, 5, 10), v1_row(1_000, 5, 11)]);
    }
}
