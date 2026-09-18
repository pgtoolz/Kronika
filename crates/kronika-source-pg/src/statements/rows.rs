//! Map captured counters to each supported registry layout without changing field order.

use super::StatementsRow;
use crate::intern_opt as opt;
use kronika_registry::pg_stat_statements::{
    PgStatStatementsV1, PgStatStatementsV2, PgStatStatementsV3, PgStatStatementsV4,
    PgStatStatementsV5, PgStatStatementsV6,
};
use kronika_registry::{StrId, Ts};

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
