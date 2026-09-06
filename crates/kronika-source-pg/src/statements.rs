//! `pg_stat_statements` collection for types `1_002_001`..`1_002_006`.
//!
//! The counters are instance-wide, so one connection to any database that has
//! the extension installed sees every statement; `dbid` says where each one
//! ran. The layout follows the extension version rather than the server major,
//! because the extension can be pinned independently.
//!
//! Query text is capped in SQL before it crosses the connection. Rows whose
//! `queryid` is privilege-masked are omitted because they have no usable
//! identity.

use kronika_registry::pg_stat_statements::{
    PgStatStatementsV1, PgStatStatementsV2, PgStatStatementsV3, PgStatStatementsV4,
    PgStatStatementsV5, PgStatStatementsV6,
};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::extension::{ExtensionSchema, ExtensionVersion, InventoryEntry, parse_version};
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

/// The extension name to look for.
pub const EXTENSION: &str = "pg_stat_statements";

/// The `pg_stat_statements` layout selected by the extension version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementsVersion {
    /// Extension 1.5-1.7: type `1_002_001`.
    V1,
    /// Extension 1.8: type `1_002_002` (planning columns, WAL columns).
    V2,
    /// Extension 1.9: type `1_002_003` (adds `toplevel`).
    V3,
    /// Extension 1.10: type `1_002_004` (adds temp-block timing and JIT).
    V4,
    /// Extension 1.11: type `1_002_005` (renamed shared-block timing, adds
    /// local-block timing, JIT deforming, and the statistics timestamps).
    V5,
    /// Extension 1.12: type `1_002_006` (adds `wal_buffers_full` and the
    /// parallel-worker counters).
    V6,
}

/// Select the layout for an installed extension version.
///
/// `PostgreSQL` 10's official `pg_stat_statements--1.4.sql` declares the V1
/// 23-column output: user/database/query ids, query text, calls, five execution
/// timings, rows, ten block counters, and two block timings. Its official
/// `pg_stat_statements--1.4--1.5.sql` update changes only
/// `pg_stat_statements_reset` privileges, so 1.5 has that exact layout. Version
/// 1.4 remains only as an upgrade source below the supported extension floor:
/// `PostgreSQL` 10's `pg_stat_statements.control` sets `default_version = '1.5'`.
#[must_use]
pub const fn statements_version(extension: ExtensionVersion) -> Option<StatementsVersion> {
    if extension.major != 1 {
        return None;
    }
    match extension.minor {
        5..=7 => Some(StatementsVersion::V1),
        8 => Some(StatementsVersion::V2),
        9 => Some(StatementsVersion::V3),
        10 => Some(StatementsVersion::V4),
        11 => Some(StatementsVersion::V5),
        minor if minor >= 12 => Some(StatementsVersion::V6),
        _ => None,
    }
}

/// Whether the layout carries the planning and WAL columns (1.8+).
const fn since_v2(version: StatementsVersion) -> bool {
    !matches!(version, StatementsVersion::V1)
}

/// Whether the layout carries `toplevel` (1.9+).
const fn since_v3(version: StatementsVersion) -> bool {
    !matches!(version, StatementsVersion::V1 | StatementsVersion::V2)
}

/// Whether the layout carries the temp-block timing and JIT columns (1.10+).
const fn since_v4(version: StatementsVersion) -> bool {
    matches!(
        version,
        StatementsVersion::V4 | StatementsVersion::V5 | StatementsVersion::V6
    )
}

/// Whether the layout carries the renamed block timing and the statistics
/// timestamps (1.11+).
const fn since_v5(version: StatementsVersion) -> bool {
    matches!(version, StatementsVersion::V5 | StatementsVersion::V6)
}

/// The columns every layout shares.
const COMMON_COLUMNS: &str = "s.queryid, s.userid, s.dbid, left(s.query, 65536) AS query, \
     d.datname::text AS datname, r.rolname::text AS usename, \
     s.calls, s.rows, \
     s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
     s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
     s.temp_blks_read, s.temp_blks_written, \
     (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us";

/// A discovered, usable `pg_stat_statements` interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementsCapability {
    /// Registry layout selected from the extension version.
    pub version: StatementsVersion,
    /// Schema containing the discovered reader function.
    pub schema: ExtensionSchema,
}

/// Select a statement interface only after catalog discovery proved it usable.
#[must_use]
pub fn capability(entry: &InventoryEntry, server_major: u32) -> Option<StatementsCapability> {
    if entry.name != EXTENSION
        || !entry.schema_usable
        || !entry.full_visibility
        || !entry.statements_reader
    {
        return None;
    }
    let version = parse_version(&entry.extversion).and_then(statements_version)?;
    if server_major >= 14 && matches!(version, StatementsVersion::V1 | StatementsVersion::V2) {
        return None;
    }
    Some(StatementsCapability {
        version,
        schema: entry.schema.clone(),
    })
}

/// The SQL for one layout.
#[must_use]
pub fn statements_query(version: StatementsVersion, schema: &ExtensionSchema) -> String {
    let mut columns = String::from(COMMON_COLUMNS);
    if since_v2(version) {
        columns.push_str(
            ", s.plans, s.total_exec_time, s.total_plan_time, \
             s.min_exec_time, s.max_exec_time, s.mean_exec_time, s.stddev_exec_time, \
             s.min_plan_time, s.max_plan_time, s.mean_plan_time, s.stddev_plan_time, \
             s.wal_records, s.wal_fpi, s.wal_bytes::int8 AS wal_bytes",
        );
    } else {
        columns.push_str(
            ", s.total_time AS total_exec_time, s.min_time AS min_exec_time, \
             s.max_time AS max_exec_time, s.mean_time AS mean_exec_time, \
             s.stddev_time AS stddev_exec_time",
        );
    }
    if since_v5(version) {
        columns.push_str(
            ", s.shared_blk_read_time, s.shared_blk_write_time, \
             s.local_blk_read_time, s.local_blk_write_time, \
             s.jit_deform_count, s.jit_deform_time, \
             (extract(epoch from s.stats_since) * 1e6)::int8 AS stats_since_us, \
             (extract(epoch from s.minmax_stats_since) * 1e6)::int8 AS minmax_stats_since_us",
        );
    } else {
        columns.push_str(
            ", s.blk_read_time AS shared_blk_read_time, \
             s.blk_write_time AS shared_blk_write_time",
        );
    }
    if since_v3(version) {
        columns.push_str(", s.toplevel");
    }
    if since_v4(version) {
        columns.push_str(
            ", s.temp_blk_read_time, s.temp_blk_write_time, \
             s.jit_functions, s.jit_generation_time, \
             s.jit_inlining_count, s.jit_inlining_time, \
             s.jit_optimization_count, s.jit_optimization_time, \
             s.jit_emission_count, s.jit_emission_time",
        );
    }
    if matches!(version, StatementsVersion::V6) {
        columns.push_str(
            ", s.wal_buffers_full, s.parallel_workers_to_launch, s.parallel_workers_launched",
        );
    }
    let source = schema.qualify(EXTENSION);
    let order = if since_v3(version) {
        "s.dbid, s.userid, s.queryid, s.toplevel"
    } else {
        "s.dbid, s.userid, s.queryid"
    };
    format!(
        "{}{columns} FROM {source}(true) s \
         LEFT JOIN pg_catalog.pg_database d ON d.oid = s.dbid \
         LEFT JOIN pg_catalog.pg_roles r ON r.oid = s.userid \
         WHERE s.queryid IS NOT NULL ORDER BY {order}",
        marked!("SELECT ")
    )
}

/// One raw `pg_stat_statements` row, a version-agnostic superset.
///
/// The timing fields use the newest names; a layout that spells them
/// differently is served by the same value. Columns absent from the extension
/// version are `None`. See [`PgStatStatementsV6`] for column meanings.
#[derive(Debug, Clone)]
pub struct StatementsRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Query id; `None` when privilege-masked.
    pub queryid: Option<i64>,
    /// Role oid the statement ran as.
    pub userid: u32,
    /// Database oid the statement ran in.
    pub dbid: u32,
    /// Whether the statement ran at the top level (V3+).
    pub toplevel: Option<bool>,
    /// Database name resolved from `dbid`.
    pub datname: Option<String>,
    /// Role name resolved from `userid`.
    pub usename: Option<String>,
    /// Statement text, bounded in SQL; `None` when `PostgreSQL` masks it.
    pub query: Option<String>,
    /// Executions.
    pub calls: i64,
    /// Rows retrieved or affected.
    pub rows: i64,
    /// Plannings (V2+).
    pub plans: Option<i64>,
    /// Total execution time, ms.
    pub total_exec_time: f64,
    /// Total planning time, ms (V2+).
    pub total_plan_time: Option<f64>,
    /// Minimum execution time, ms.
    pub min_exec_time: f64,
    /// Maximum execution time, ms.
    pub max_exec_time: f64,
    /// Mean execution time, ms.
    pub mean_exec_time: f64,
    /// Standard deviation of execution time, ms.
    pub stddev_exec_time: f64,
    /// Minimum planning time, ms (V2+).
    pub min_plan_time: Option<f64>,
    /// Maximum planning time, ms (V2+).
    pub max_plan_time: Option<f64>,
    /// Mean planning time, ms (V2+).
    pub mean_plan_time: Option<f64>,
    /// Standard deviation of planning time, ms (V2+).
    pub stddev_plan_time: Option<f64>,
    /// Shared-block buffer hits.
    pub shared_blks_hit: i64,
    /// Shared blocks read.
    pub shared_blks_read: i64,
    /// Shared blocks dirtied.
    pub shared_blks_dirtied: i64,
    /// Shared blocks written.
    pub shared_blks_written: i64,
    /// Local-block buffer hits.
    pub local_blks_hit: i64,
    /// Local blocks read.
    pub local_blks_read: i64,
    /// Local blocks dirtied.
    pub local_blks_dirtied: i64,
    /// Local blocks written.
    pub local_blks_written: i64,
    /// Temp blocks read.
    pub temp_blks_read: i64,
    /// Temp blocks written.
    pub temp_blks_written: i64,
    /// Time reading shared blocks, ms (`blk_read_time` before 1.11).
    pub shared_blk_read_time: f64,
    /// Time writing shared blocks, ms (`blk_write_time` before 1.11).
    pub shared_blk_write_time: f64,
    /// Time reading local blocks, ms (V5+).
    pub local_blk_read_time: Option<f64>,
    /// Time writing local blocks, ms (V5+).
    pub local_blk_write_time: Option<f64>,
    /// Time reading temp blocks, ms (V4+).
    pub temp_blk_read_time: Option<f64>,
    /// Time writing temp blocks, ms (V4+).
    pub temp_blk_write_time: Option<f64>,
    /// WAL records generated (V2+).
    pub wal_records: Option<i64>,
    /// WAL full-page images generated (V2+).
    pub wal_fpi: Option<i64>,
    /// WAL bytes generated (V2+).
    pub wal_bytes: Option<i64>,
    /// Waits on a full WAL buffer (V6).
    pub wal_buffers_full: Option<i64>,
    /// JIT-compiled functions (V4+).
    pub jit_functions: Option<i64>,
    /// JIT generation time, ms (V4+).
    pub jit_generation_time: Option<f64>,
    /// JIT inlining passes (V4+).
    pub jit_inlining_count: Option<i64>,
    /// JIT inlining time, ms (V4+).
    pub jit_inlining_time: Option<f64>,
    /// JIT optimization passes (V4+).
    pub jit_optimization_count: Option<i64>,
    /// JIT optimization time, ms (V4+).
    pub jit_optimization_time: Option<f64>,
    /// JIT code emissions (V4+).
    pub jit_emission_count: Option<i64>,
    /// JIT emission time, ms (V4+).
    pub jit_emission_time: Option<f64>,
    /// JIT tuple-deforming passes (V5+).
    pub jit_deform_count: Option<i64>,
    /// JIT tuple-deforming time, ms (V5+).
    pub jit_deform_time: Option<f64>,
    /// Parallel workers planned for launch (V6).
    pub parallel_workers_to_launch: Option<i64>,
    /// Parallel workers launched (V6).
    pub parallel_workers_launched: Option<i64>,
    /// Start of this row's statistics, unix microseconds; `None` before V5.
    pub stats_since: Option<i64>,
    /// Last min/max reset, unix microseconds; `None` before V5.
    pub minmax_stats_since: Option<i64>,
}

/// Build a `1_002_006` row (extension 1.12 layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v6<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV6, E> {
    Ok(PgStatStatementsV6 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        toplevel: row.toplevel.unwrap_or(true),
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        plans: row.plans.unwrap_or(0),
        total_exec_time: row.total_exec_time,
        total_plan_time: row.total_plan_time.unwrap_or(0.0),
        min_exec_time: row.min_exec_time,
        max_exec_time: row.max_exec_time,
        mean_exec_time: row.mean_exec_time,
        stddev_exec_time: row.stddev_exec_time,
        min_plan_time: row.min_plan_time.unwrap_or(0.0),
        max_plan_time: row.max_plan_time.unwrap_or(0.0),
        mean_plan_time: row.mean_plan_time.unwrap_or(0.0),
        stddev_plan_time: row.stddev_plan_time.unwrap_or(0.0),
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        shared_blk_read_time: row.shared_blk_read_time,
        shared_blk_write_time: row.shared_blk_write_time,
        local_blk_read_time: row.local_blk_read_time.unwrap_or(0.0),
        local_blk_write_time: row.local_blk_write_time.unwrap_or(0.0),
        temp_blk_read_time: row.temp_blk_read_time.unwrap_or(0.0),
        temp_blk_write_time: row.temp_blk_write_time.unwrap_or(0.0),
        wal_records: row.wal_records.unwrap_or(0),
        wal_fpi: row.wal_fpi.unwrap_or(0),
        wal_bytes: row.wal_bytes.unwrap_or(0),
        wal_buffers_full: row.wal_buffers_full.unwrap_or(0),
        jit_functions: row.jit_functions.unwrap_or(0),
        jit_generation_time: row.jit_generation_time.unwrap_or(0.0),
        jit_inlining_count: row.jit_inlining_count.unwrap_or(0),
        jit_inlining_time: row.jit_inlining_time.unwrap_or(0.0),
        jit_optimization_count: row.jit_optimization_count.unwrap_or(0),
        jit_optimization_time: row.jit_optimization_time.unwrap_or(0.0),
        jit_emission_count: row.jit_emission_count.unwrap_or(0),
        jit_emission_time: row.jit_emission_time.unwrap_or(0.0),
        jit_deform_count: row.jit_deform_count.unwrap_or(0),
        jit_deform_time: row.jit_deform_time.unwrap_or(0.0),
        parallel_workers_to_launch: row.parallel_workers_to_launch.unwrap_or(0),
        parallel_workers_launched: row.parallel_workers_launched.unwrap_or(0),
        stats_since: Ts(row.stats_since.unwrap_or(0)),
        minmax_stats_since: Ts(row.minmax_stats_since.unwrap_or(0)),
    })
}

/// Build a `1_002_005` row (extension 1.11 layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v5<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV5, E> {
    Ok(PgStatStatementsV5 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        toplevel: row.toplevel.unwrap_or(true),
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        plans: row.plans.unwrap_or(0),
        total_exec_time: row.total_exec_time,
        total_plan_time: row.total_plan_time.unwrap_or(0.0),
        min_exec_time: row.min_exec_time,
        max_exec_time: row.max_exec_time,
        mean_exec_time: row.mean_exec_time,
        stddev_exec_time: row.stddev_exec_time,
        min_plan_time: row.min_plan_time.unwrap_or(0.0),
        max_plan_time: row.max_plan_time.unwrap_or(0.0),
        mean_plan_time: row.mean_plan_time.unwrap_or(0.0),
        stddev_plan_time: row.stddev_plan_time.unwrap_or(0.0),
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        shared_blk_read_time: row.shared_blk_read_time,
        shared_blk_write_time: row.shared_blk_write_time,
        local_blk_read_time: row.local_blk_read_time.unwrap_or(0.0),
        local_blk_write_time: row.local_blk_write_time.unwrap_or(0.0),
        temp_blk_read_time: row.temp_blk_read_time.unwrap_or(0.0),
        temp_blk_write_time: row.temp_blk_write_time.unwrap_or(0.0),
        wal_records: row.wal_records.unwrap_or(0),
        wal_fpi: row.wal_fpi.unwrap_or(0),
        wal_bytes: row.wal_bytes.unwrap_or(0),
        jit_functions: row.jit_functions.unwrap_or(0),
        jit_generation_time: row.jit_generation_time.unwrap_or(0.0),
        jit_inlining_count: row.jit_inlining_count.unwrap_or(0),
        jit_inlining_time: row.jit_inlining_time.unwrap_or(0.0),
        jit_optimization_count: row.jit_optimization_count.unwrap_or(0),
        jit_optimization_time: row.jit_optimization_time.unwrap_or(0.0),
        jit_emission_count: row.jit_emission_count.unwrap_or(0),
        jit_emission_time: row.jit_emission_time.unwrap_or(0.0),
        jit_deform_count: row.jit_deform_count.unwrap_or(0),
        jit_deform_time: row.jit_deform_time.unwrap_or(0.0),
        stats_since: Ts(row.stats_since.unwrap_or(0)),
        minmax_stats_since: Ts(row.minmax_stats_since.unwrap_or(0)),
    })
}

/// Build a `1_002_004` row (extension 1.10 layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v4<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV4, E> {
    Ok(PgStatStatementsV4 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        toplevel: row.toplevel.unwrap_or(true),
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        plans: row.plans.unwrap_or(0),
        total_exec_time: row.total_exec_time,
        total_plan_time: row.total_plan_time.unwrap_or(0.0),
        min_exec_time: row.min_exec_time,
        max_exec_time: row.max_exec_time,
        mean_exec_time: row.mean_exec_time,
        stddev_exec_time: row.stddev_exec_time,
        min_plan_time: row.min_plan_time.unwrap_or(0.0),
        max_plan_time: row.max_plan_time.unwrap_or(0.0),
        mean_plan_time: row.mean_plan_time.unwrap_or(0.0),
        stddev_plan_time: row.stddev_plan_time.unwrap_or(0.0),
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        blk_read_time: row.shared_blk_read_time,
        blk_write_time: row.shared_blk_write_time,
        temp_blk_read_time: row.temp_blk_read_time.unwrap_or(0.0),
        temp_blk_write_time: row.temp_blk_write_time.unwrap_or(0.0),
        wal_records: row.wal_records.unwrap_or(0),
        wal_fpi: row.wal_fpi.unwrap_or(0),
        wal_bytes: row.wal_bytes.unwrap_or(0),
        jit_functions: row.jit_functions.unwrap_or(0),
        jit_generation_time: row.jit_generation_time.unwrap_or(0.0),
        jit_inlining_count: row.jit_inlining_count.unwrap_or(0),
        jit_inlining_time: row.jit_inlining_time.unwrap_or(0.0),
        jit_optimization_count: row.jit_optimization_count.unwrap_or(0),
        jit_optimization_time: row.jit_optimization_time.unwrap_or(0.0),
        jit_emission_count: row.jit_emission_count.unwrap_or(0),
        jit_emission_time: row.jit_emission_time.unwrap_or(0.0),
    })
}

/// Build a `1_002_003` row (extension 1.9 layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v3<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV3, E> {
    Ok(PgStatStatementsV3 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        toplevel: row.toplevel.unwrap_or(true),
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        plans: row.plans.unwrap_or(0),
        total_exec_time: row.total_exec_time,
        total_plan_time: row.total_plan_time.unwrap_or(0.0),
        min_exec_time: row.min_exec_time,
        max_exec_time: row.max_exec_time,
        mean_exec_time: row.mean_exec_time,
        stddev_exec_time: row.stddev_exec_time,
        min_plan_time: row.min_plan_time.unwrap_or(0.0),
        max_plan_time: row.max_plan_time.unwrap_or(0.0),
        mean_plan_time: row.mean_plan_time.unwrap_or(0.0),
        stddev_plan_time: row.stddev_plan_time.unwrap_or(0.0),
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        blk_read_time: row.shared_blk_read_time,
        blk_write_time: row.shared_blk_write_time,
        wal_records: row.wal_records.unwrap_or(0),
        wal_fpi: row.wal_fpi.unwrap_or(0),
        wal_bytes: row.wal_bytes.unwrap_or(0),
    })
}

/// Build a `1_002_002` row (extension 1.8 layout, no `toplevel`).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v2<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV2, E> {
    Ok(PgStatStatementsV2 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        plans: row.plans.unwrap_or(0),
        total_exec_time: row.total_exec_time,
        total_plan_time: row.total_plan_time.unwrap_or(0.0),
        min_exec_time: row.min_exec_time,
        max_exec_time: row.max_exec_time,
        mean_exec_time: row.mean_exec_time,
        stddev_exec_time: row.stddev_exec_time,
        min_plan_time: row.min_plan_time.unwrap_or(0.0),
        max_plan_time: row.max_plan_time.unwrap_or(0.0),
        mean_plan_time: row.mean_plan_time.unwrap_or(0.0),
        stddev_plan_time: row.stddev_plan_time.unwrap_or(0.0),
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        blk_read_time: row.shared_blk_read_time,
        blk_write_time: row.shared_blk_write_time,
        wal_records: row.wal_records.unwrap_or(0),
        wal_fpi: row.wal_fpi.unwrap_or(0),
        wal_bytes: row.wal_bytes.unwrap_or(0),
    })
}

/// Build a `1_002_001` row (extension 1.5-1.7 layout, no planning columns).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v1<E>(
    row: &StatementsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatStatementsV1, E> {
    Ok(PgStatStatementsV1 {
        ts: Ts(row.ts),
        queryid: row.queryid,
        userid: row.userid,
        dbid: row.dbid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        calls: row.calls,
        rows: row.rows,
        total_time: row.total_exec_time,
        min_time: row.min_exec_time,
        max_time: row.max_exec_time,
        mean_time: row.mean_exec_time,
        stddev_time: row.stddev_exec_time,
        shared_blks_hit: row.shared_blks_hit,
        shared_blks_read: row.shared_blks_read,
        shared_blks_dirtied: row.shared_blks_dirtied,
        shared_blks_written: row.shared_blks_written,
        local_blks_hit: row.local_blks_hit,
        local_blks_read: row.local_blks_read,
        local_blks_dirtied: row.local_blks_dirtied,
        local_blks_written: row.local_blks_written,
        temp_blks_read: row.temp_blks_read,
        temp_blks_written: row.temp_blks_written,
        blk_read_time: row.shared_blk_read_time,
        blk_write_time: row.shared_blk_write_time,
    })
}

/// Read a raw row using the layout's column set.
fn row_from_pg(
    row: query::IndexedRow<'_>,
    version: StatementsVersion,
) -> anyhow::Result<StatementsRow> {
    let planning = since_v2(version);
    let jit = since_v4(version);
    let renamed = since_v5(version);
    Ok(StatementsRow {
        ts: row.try_get("ts_us")?,
        queryid: Some(row.try_get("queryid")?),
        userid: row.try_get("userid")?,
        dbid: row.try_get("dbid")?,
        toplevel: since_v3(version)
            .then(|| row.try_get("toplevel"))
            .transpose()?,
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        query: row.try_get("query")?,
        calls: row.try_get("calls")?,
        rows: row.try_get("rows")?,
        plans: planning.then(|| row.try_get("plans")).transpose()?,
        total_exec_time: row.try_get("total_exec_time")?,
        total_plan_time: planning
            .then(|| row.try_get("total_plan_time"))
            .transpose()?,
        min_exec_time: row.try_get("min_exec_time")?,
        max_exec_time: row.try_get("max_exec_time")?,
        mean_exec_time: row.try_get("mean_exec_time")?,
        stddev_exec_time: row.try_get("stddev_exec_time")?,
        min_plan_time: planning.then(|| row.try_get("min_plan_time")).transpose()?,
        max_plan_time: planning.then(|| row.try_get("max_plan_time")).transpose()?,
        mean_plan_time: planning
            .then(|| row.try_get("mean_plan_time"))
            .transpose()?,
        stddev_plan_time: planning
            .then(|| row.try_get("stddev_plan_time"))
            .transpose()?,
        shared_blks_hit: row.try_get("shared_blks_hit")?,
        shared_blks_read: row.try_get("shared_blks_read")?,
        shared_blks_dirtied: row.try_get("shared_blks_dirtied")?,
        shared_blks_written: row.try_get("shared_blks_written")?,
        local_blks_hit: row.try_get("local_blks_hit")?,
        local_blks_read: row.try_get("local_blks_read")?,
        local_blks_dirtied: row.try_get("local_blks_dirtied")?,
        local_blks_written: row.try_get("local_blks_written")?,
        temp_blks_read: row.try_get("temp_blks_read")?,
        temp_blks_written: row.try_get("temp_blks_written")?,
        shared_blk_read_time: row.try_get("shared_blk_read_time")?,
        shared_blk_write_time: row.try_get("shared_blk_write_time")?,
        local_blk_read_time: renamed
            .then(|| row.try_get("local_blk_read_time"))
            .transpose()?,
        local_blk_write_time: renamed
            .then(|| row.try_get("local_blk_write_time"))
            .transpose()?,
        temp_blk_read_time: jit.then(|| row.try_get("temp_blk_read_time")).transpose()?,
        temp_blk_write_time: jit
            .then(|| row.try_get("temp_blk_write_time"))
            .transpose()?,
        wal_records: planning.then(|| row.try_get("wal_records")).transpose()?,
        wal_fpi: planning.then(|| row.try_get("wal_fpi")).transpose()?,
        wal_bytes: planning.then(|| row.try_get("wal_bytes")).transpose()?,
        wal_buffers_full: matches!(version, StatementsVersion::V6)
            .then(|| row.try_get("wal_buffers_full"))
            .transpose()?,
        jit_functions: jit.then(|| row.try_get("jit_functions")).transpose()?,
        jit_generation_time: jit
            .then(|| row.try_get("jit_generation_time"))
            .transpose()?,
        jit_inlining_count: jit.then(|| row.try_get("jit_inlining_count")).transpose()?,
        jit_inlining_time: jit.then(|| row.try_get("jit_inlining_time")).transpose()?,
        jit_optimization_count: jit
            .then(|| row.try_get("jit_optimization_count"))
            .transpose()?,
        jit_optimization_time: jit
            .then(|| row.try_get("jit_optimization_time"))
            .transpose()?,
        jit_emission_count: jit.then(|| row.try_get("jit_emission_count")).transpose()?,
        jit_emission_time: jit.then(|| row.try_get("jit_emission_time")).transpose()?,
        jit_deform_count: renamed
            .then(|| row.try_get("jit_deform_count"))
            .transpose()?,
        jit_deform_time: renamed
            .then(|| row.try_get("jit_deform_time"))
            .transpose()?,
        parallel_workers_to_launch: matches!(version, StatementsVersion::V6)
            .then(|| row.try_get("parallel_workers_to_launch"))
            .transpose()?,
        parallel_workers_launched: matches!(version, StatementsVersion::V6)
            .then(|| row.try_get("parallel_workers_launched"))
            .transpose()?,
        stats_since: if renamed {
            row.try_get("stats_since_us")?
        } else {
            None
        },
        minmax_stats_since: if renamed {
            row.try_get("minmax_stats_since_us")?
        } else {
            None
        },
    })
}

/// Collect every visible statement the extension is holding counters for.
///
/// # Errors
/// Returns a [`BatchError`] when the query, row decoding, or batch sink fails.
pub async fn collect_statements<E>(
    session: Session<'_>,
    capability: &StatementsCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<StatementsRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = capability.version;
    query::read_batched(
        session,
        &statements_query(version, &capability.schema),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| row_from_pg(row, version),
        |_row| 0,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests;
