//! Decode and stream each `pg_store_plans` implementation without merging their schemas.

use super::{DatasentinelRow, OsscRow, StorePlansCapability, VadvRow, store_plans_query};
use crate::Session;
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use tokio_postgres::types::Type;

fn common_counters_from_pg(
    row: query::IndexedRow<'_>,
    predecoded_calls: Option<i64>,
) -> anyhow::Result<OsscRow> {
    Ok(OsscRow {
        ts: row.try_get("ts_us")?,
        queryid: row.try_get("queryid")?,
        planid: row.try_get("planid")?,
        userid: row.try_get("userid")?,
        dbid: row.try_get("dbid")?,
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        plan: row.try_get("plan")?,
        calls: match predecoded_calls {
            Some(calls) => calls,
            None => row.try_get("calls")?,
        },
        total_time: row.try_get("total_time")?,
        min_time: row.try_get("min_time")?,
        max_time: row.try_get("max_time")?,
        mean_time: row.try_get("mean_time")?,
        stddev_time: row.try_get("stddev_time")?,
        rows: row.try_get("rows")?,
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
        local_blk_read_time: row.try_get("local_blk_read_time")?,
        local_blk_write_time: row.try_get("local_blk_write_time")?,
        temp_blk_read_time: row.try_get("temp_blk_read_time")?,
        temp_blk_write_time: row.try_get("temp_blk_write_time")?,
        first_call: 0,
        last_call: 0,
    })
}

fn ossc_row_from_pg(row: query::IndexedRow<'_>) -> anyhow::Result<OsscRow> {
    let mut counters = common_counters_from_pg(row, None)?;
    counters.first_call = row.try_get("first_call_us")?;
    counters.last_call = row.try_get("last_call_us")?;
    Ok(counters)
}

fn datasentinel_row_from_pg(row: query::IndexedRow<'_>) -> anyhow::Result<DatasentinelRow> {
    // Keep this flavor's calls-first diagnostic when several columns are malformed.
    let calls = row.try_get("calls")?;
    Ok(DatasentinelRow {
        // Datasentinel timestamps are nullable and belong to its extended shape.
        // The shared OSSC counter record keeps its unused timestamp slots at zero.
        base: common_counters_from_pg(row, Some(calls))?,
        relids: row.try_get("relids")?,
        cmd_type: row.try_get("cmd_type")?,
        first_call: row.try_get("first_call_us")?,
        last_call: row.try_get("last_call_us")?,
    })
}

fn vadv_row_from_pg(row: query::IndexedRow<'_>) -> anyhow::Result<VadvRow> {
    Ok(VadvRow {
        ts: row.try_get("ts_us")?,
        userid: row.try_get("userid")?,
        dbid: row.try_get("dbid")?,
        queryid: row.try_get("queryid")?,
        planid: row.try_get("planid")?,
        queryid_stat_statements: row.try_get("queryid_stat_statements")?,
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        plan: row.try_get("plan")?,
        calls: row.try_get("calls")?,
        slow_log_calls: row.try_get("slow_log_calls")?,
        total_time: row.try_get("total_time")?,
        min_time: row.try_get("min_time")?,
        max_time: row.try_get("max_time")?,
        mean_time: row.try_get("mean_time")?,
        stddev_time: row.try_get("stddev_time")?,
        rows: row.try_get("rows")?,
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
        blk_read_time: row.try_get("blk_read_time")?,
        blk_write_time: row.try_get("blk_write_time")?,
        first_call: row.try_get("first_call_us")?,
        last_call: row.try_get("last_call_us")?,
        total_plan_time: row.try_get("total_plan_time")?,
        min_plan_time: row.try_get("min_plan_time")?,
        max_plan_time: row.try_get("max_plan_time")?,
        mean_plan_time: row.try_get("mean_plan_time")?,
    })
}

/// Collect every OSSC-compatible plan entry.
///
/// # Errors
/// Returns a [`BatchError`] when the query, row decoding, or batch sink fails.
pub async fn collect_ossc<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<OsscRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        &store_plans_query(capability),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        ossc_row_from_pg,
        |_row| 0,
        sink,
    )
    .await
}

/// Collect every Datasentinel plan entry.
///
/// # Errors
/// Returns a [`BatchError`] when the query, row decoding, or batch sink fails.
pub async fn collect_datasentinel<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<DatasentinelRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        &store_plans_query(capability),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        datasentinel_row_from_pg,
        |_row| 0,
        sink,
    )
    .await
}

/// Collect every vadv entry and its bounded human-readable plan text in one set-based query.
///
/// # Errors
/// Returns a [`BatchError`] when the query, row decoding, or batch sink fails.
pub async fn collect_vadv<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<VadvRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        &store_plans_query(capability),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        vadv_row_from_pg,
        |_row| 0,
        sink,
    )
    .await
}
