//! Type `1_004_001`: `pg_store_plans`, the vadv fork (extension 2.x).
//!
//! Per-plan execution counters. The vadv fork keys rows by
//! `(userid, dbid, queryid, planid)`, where `queryid` is the extension's
//! internal query id. The statistics are instance-wide, read from the one
//! database where `CREATE EXTENSION pg_store_plans` ran. The vadv fork and the
//! ossc upstream expose different column sets and different plan-text access
//! paths, so they are separate type families (`1_004` vadv, `1_003` ossc), not
//! one layout with optional columns.
//!
//! `queryid_stat_statements` is best-effort attribution, not identity: the
//! extension overwrites it on every execution, so the value names the LAST
//! statement that ran this plan. Joining to `1_002` through it is valid only
//! under that caveat, and it stays `0` unless `compute_query_id = on`.
//!
//! `planid` identifies rows only within one instance, one server major, and
//! one extension version; it is not a portable identifier.
//! Timing columns are `f64`, so the layout derives `PartialEq` but not `Eq`.

use crate::{Section, StrId, Ts};

/// Type `1_004_001`: `pg_store_plans` (vadv fork, extension 2.x).
///
/// One row per visible plan entry of `pg_store_plans(false)`;
/// the row identity is `(userid, dbid, queryid, planid)`, matching the
/// extension's `EntryKey` and SQL function output.
/// The `*_blk_*_time` columns are `0` when `track_io_timing` is off — an
/// unmeasured zero is indistinguishable from a true zero. The `*_plan_time`
/// columns are `0` without `pg_store_plans.track_planning`.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_004_001,
    name = "pg_store_plans_vadv",
    semantics = conditional_full,
    sort_key("dbid", "userid", "queryid", "planid"),
    identity("userid", "dbid", "queryid", "planid")
)]
pub struct PgStorePlansVadvV1 {
    /// Collection time, unix microseconds; one value for all rows of a read.
    #[column(t)]
    pub ts: Ts,
    /// Role oid the statements ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statements ran in.
    #[column(l)]
    pub dbid: u32,
    /// Extension-internal query id, part of the entry identity.
    #[column(l)]
    pub queryid: i64,
    /// Plan id derived from the normalized plan representation.
    #[column(l)]
    pub planid: i64,
    /// `pg_stat_statements` query id of the LAST statement that ran this
    /// plan (overwritten by the extension per execution); `0` when
    /// `compute_query_id` is off. Best-effort bridge to section `1_002`, not
    /// part of the row identity.
    #[column(l)]
    pub queryid_stat_statements: i64,
    /// Database name resolved from `dbid`; `None` when `dbid` has no
    /// `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no
    /// `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Human-readable plan text materialized by the extension; `None` when unavailable.
    #[column(l)]
    pub plan: Option<StrId>,
    /// Executions accumulated for this plan entry.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Executions recorded through `pg_store_plans.slow_statement_duration`.
    #[column(c, unit = count)]
    pub slow_log_calls: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_time: f64,
    /// Population standard deviation of execution time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_time: f64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
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
    /// When statistics for this entry began accumulating.
    #[column(g, unit = microseconds)]
    pub first_call: Ts,
    /// When the entry was last executed.
    #[column(g, unit = microseconds)]
    pub last_call: Ts,
    /// Total planning time in milliseconds; `0` without `track_planning`.
    #[column(c, unit = milliseconds)]
    pub total_plan_time: f64,
    /// Minimum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub min_plan_time: f64,
    /// Maximum planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub max_plan_time: f64,
    /// Mean planning time in milliseconds; `0` without `track_planning`.
    #[column(g, unit = milliseconds)]
    pub mean_plan_time: f64,
}

#[cfg(test)]
#[path = "../tests/codec/pg_store_plans.rs"]
mod tests;

/// Type `1_003_001`: `pg_store_plans` (ossc upstream, extension 1.9+).
///
/// One row per plan entry, top-N by `total_time`; unlike the vadv fork the
/// upstream keys an entry by `(userid, dbid, queryid, planid)` with the real
/// 64-bit core query id, so plans stay per-statement and `queryid` joins
/// section `1_002` directly. The extension does not record entries at all
/// when `compute_query_id` is off. I/O timings are split by block class
/// (extension 1.9); every `*_time` column is `0` without `track_io_timing`.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_003_001,
    name = "pg_store_plans_ossc",
    semantics = conditional_full,
    identity("userid", "dbid", "queryid", "planid"),
    sort_key("dbid", "userid", "queryid", "planid")
)]
pub struct PgStorePlansOsscV1 {
    /// Collection time, unix microseconds; one value for all rows of a read.
    #[column(t)]
    pub ts: Ts,
    /// Core query id, part of the entry identity; joins section `1_002`.
    #[column(l)]
    pub queryid: i64,
    /// Plan id derived from the normalized plan representation.
    #[column(l)]
    pub planid: i64,
    /// Role oid the statements ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statements ran in.
    #[column(l)]
    pub dbid: u32,
    /// Database name resolved from `dbid`; `None` when `dbid` has no
    /// `pg_database` row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`; `None` when `userid` has no
    /// `pg_roles` row.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Human-readable plan text from the view, server-truncated per row; `None` when the
    /// server does not expose text for this entry.
    #[column(l)]
    pub plan: Option<StrId>,
    /// Executions accumulated for this plan entry.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_time: f64,
    /// Minimum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub min_time: f64,
    /// Maximum execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub max_time: f64,
    /// Mean execution time in milliseconds (resettable).
    #[column(g, unit = milliseconds)]
    pub mean_time: f64,
    /// Population standard deviation of execution time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_time: f64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
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
    /// Time reading shared blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub local_blk_read_time: f64,
    /// Time writing local blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub local_blk_write_time: f64,
    /// Time reading temp blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub temp_blk_write_time: f64,
    /// When statistics for this entry began accumulating.
    #[column(g, unit = microseconds)]
    pub first_call: Ts,
    /// When the entry was last executed.
    #[column(g, unit = microseconds)]
    pub last_call: Ts,
}

/// Type `1_018_001`: Datasentinel `pg_store_plans` 2.x.
///
/// This interface extends the OSSC-compatible counters with the relation OIDs
/// and command type. `relids` keeps `PostgreSQL`'s lossless `oid[]` text because
/// the segment codec has no unsigned-integer list type.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_018_001,
    name = "pg_store_plans_datasentinel",
    semantics = conditional_full,
    identity("userid", "dbid", "queryid", "planid"),
    sort_key("dbid", "userid", "queryid", "planid")
)]
pub struct PgStorePlansDatasentinelV1 {
    /// Collection time, unix microseconds; one value for all rows of a read.
    #[column(t)]
    pub ts: Ts,
    /// Core query id, part of the entry identity; joins section `1_002`.
    #[column(l)]
    pub queryid: i64,
    /// Plan id derived from the normalized plan representation.
    #[column(l)]
    pub planid: i64,
    /// Role oid the statements ran as.
    #[column(l)]
    pub userid: u32,
    /// Database oid the statements ran in.
    #[column(l)]
    pub dbid: u32,
    /// Database name resolved from `dbid`.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name resolved from `userid`.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Server-truncated human-readable plan text.
    #[column(l)]
    pub plan: Option<StrId>,
    /// Relation OIDs in `PostgreSQL` `oid[]` text form, at most 48 elements.
    #[column(l)]
    pub relids: Option<StrId>,
    /// Command type reported by the extension.
    #[column(l)]
    pub cmd_type: Option<StrId>,
    /// Executions accumulated for this plan entry.
    #[column(c, unit = count)]
    pub calls: i64,
    /// Total execution time in milliseconds.
    #[column(c, unit = milliseconds)]
    pub total_time: f64,
    /// Minimum execution time in milliseconds.
    #[column(g, unit = milliseconds)]
    pub min_time: f64,
    /// Maximum execution time in milliseconds.
    #[column(g, unit = milliseconds)]
    pub max_time: f64,
    /// Mean execution time in milliseconds.
    #[column(g, unit = milliseconds)]
    pub mean_time: f64,
    /// Population standard deviation of execution time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub stddev_time: f64,
    /// Rows retrieved or affected.
    #[column(c, unit = count)]
    pub rows: i64,
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
    /// Time reading shared blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub local_blk_read_time: f64,
    /// Time writing local blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub local_blk_write_time: f64,
    /// Time reading temp blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub temp_blk_read_time: f64,
    /// Time writing temp blocks, milliseconds.
    #[column(c, unit = milliseconds)]
    pub temp_blk_write_time: f64,
    /// When statistics began; `None` while the first call is in flight.
    #[column(g, unit = microseconds)]
    pub first_call: Option<Ts>,
    /// Last completed execution; `None` before the first completion.
    #[column(g, unit = microseconds)]
    pub last_call: Option<Ts>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_store_plans_ossc.rs"]
mod ossc_tests;

#[cfg(test)]
#[path = "../tests/codec/pg_store_plans_datasentinel.rs"]
mod datasentinel_tests;
