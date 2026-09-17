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
#[path = "../tests/codec/pg_stat_statements.rs"]
mod tests;
