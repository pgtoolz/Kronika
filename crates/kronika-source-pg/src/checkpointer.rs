//! `pg_stat_checkpointer` collection for types `1_017_001` / `1_017_002`.

use kronika_registry::Ts;
use kronika_registry::pg_stat_checkpointer::{PgStatCheckpointerV1, PgStatCheckpointerV2};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

/// Layout selected by the `PostgreSQL` major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointerVersion {
    /// `PostgreSQL` 17.
    V1,
    /// `PostgreSQL` 18, with `num_done` and `slru_written`.
    V2,
}

/// Select the layout, or `None` before the view appeared in `PostgreSQL` 17.
#[must_use]
pub const fn checkpointer_version(major: u32) -> Option<CheckpointerVersion> {
    if major >= 18 {
        Some(CheckpointerVersion::V2)
    } else if major >= 17 {
        Some(CheckpointerVersion::V1)
    } else {
        None
    }
}

/// SQL for one server layout.
#[must_use]
pub const fn checkpointer_query(version: CheckpointerVersion) -> &'static str {
    match version {
        CheckpointerVersion::V1 => marked!(
            "SELECT num_timed, num_requested, restartpoints_timed, restartpoints_req, \
             restartpoints_done, write_time, sync_time, buffers_written, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_checkpointer"
        ),
        CheckpointerVersion::V2 => marked!(
            "SELECT num_timed, num_requested, num_done, restartpoints_timed, \
             restartpoints_req, restartpoints_done, write_time, sync_time, \
             buffers_written, slru_written, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_checkpointer"
        ),
    }
}

/// One versioned checkpointer snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CheckpointerSnapshot {
    /// `PostgreSQL` 17 layout.
    V1(PgStatCheckpointerV1),
    /// `PostgreSQL` 18 layout.
    V2(PgStatCheckpointerV2),
}

/// Collect the singleton row, or `None` before `PostgreSQL` 17.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect_checkpointer(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
) -> anyhow::Result<Option<CheckpointerSnapshot>> {
    let Some(version) = checkpointer_version(major) else {
        return Ok(None);
    };
    query::read_one(
        session,
        checkpointer_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(match version {
                CheckpointerVersion::V1 => CheckpointerSnapshot::V1(PgStatCheckpointerV1 {
                    ts: Ts(row.try_get("ts_us")?),
                    num_timed: row.try_get("num_timed")?,
                    num_requested: row.try_get("num_requested")?,
                    restartpoints_timed: row.try_get("restartpoints_timed")?,
                    restartpoints_req: row.try_get("restartpoints_req")?,
                    restartpoints_done: row.try_get("restartpoints_done")?,
                    write_time: row.try_get("write_time")?,
                    sync_time: row.try_get("sync_time")?,
                    buffers_written: row.try_get("buffers_written")?,
                    stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
                }),
                CheckpointerVersion::V2 => CheckpointerSnapshot::V2(PgStatCheckpointerV2 {
                    ts: Ts(row.try_get("ts_us")?),
                    num_timed: row.try_get("num_timed")?,
                    num_requested: row.try_get("num_requested")?,
                    num_done: row.try_get("num_done")?,
                    restartpoints_timed: row.try_get("restartpoints_timed")?,
                    restartpoints_req: row.try_get("restartpoints_req")?,
                    restartpoints_done: row.try_get("restartpoints_done")?,
                    write_time: row.try_get("write_time")?,
                    sync_time: row.try_get("sync_time")?,
                    buffers_written: row.try_get("buffers_written")?,
                    slru_written: row.try_get("slru_written")?,
                    stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
                }),
            })
        },
    )
    .await
    .map(Some)
}

#[cfg(test)]
mod tests;
