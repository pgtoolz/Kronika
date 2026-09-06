//! `pg_stat_bgwriter` collection for types `1_006_001` / `1_006_002`.

use kronika_registry::Ts;
use kronika_registry::pg_stat_bgwriter::{PgStatBgwriterV1, PgStatBgwriterV2};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

/// Layout selected by the `PostgreSQL` major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgwriterVersion {
    /// `PostgreSQL` 10-16, with checkpoint and backend-write counters.
    V1,
    /// `PostgreSQL` 17-18, after those counters moved elsewhere.
    V2,
}

/// Select the `pg_stat_bgwriter` layout for a supported server.
#[must_use]
pub const fn bgwriter_version(major: u32) -> BgwriterVersion {
    if major >= 17 {
        BgwriterVersion::V2
    } else {
        BgwriterVersion::V1
    }
}

/// SQL for one server layout.
#[must_use]
pub const fn bgwriter_query(version: BgwriterVersion) -> &'static str {
    match version {
        BgwriterVersion::V1 => marked!(
            "SELECT checkpoints_timed, checkpoints_req, checkpoint_write_time, \
             checkpoint_sync_time, buffers_checkpoint, buffers_clean, maxwritten_clean, \
             buffers_backend, buffers_backend_fsync, buffers_alloc, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_bgwriter"
        ),
        BgwriterVersion::V2 => marked!(
            "SELECT buffers_clean, maxwritten_clean, buffers_alloc, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_bgwriter"
        ),
    }
}

/// One versioned background-writer snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BgwriterSnapshot {
    /// `PostgreSQL` 10-16 layout.
    V1(PgStatBgwriterV1),
    /// `PostgreSQL` 17-18 layout.
    V2(PgStatBgwriterV2),
}

/// Collect the singleton `pg_stat_bgwriter` row using one unnamed statement.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect_bgwriter(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
) -> anyhow::Result<BgwriterSnapshot> {
    let version = bgwriter_version(major);
    query::read_one(
        session,
        bgwriter_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(match version {
                BgwriterVersion::V1 => BgwriterSnapshot::V1(PgStatBgwriterV1 {
                    ts: Ts(row.try_get("ts_us")?),
                    checkpoints_timed: row.try_get("checkpoints_timed")?,
                    checkpoints_req: row.try_get("checkpoints_req")?,
                    checkpoint_write_time: row.try_get("checkpoint_write_time")?,
                    checkpoint_sync_time: row.try_get("checkpoint_sync_time")?,
                    buffers_checkpoint: row.try_get("buffers_checkpoint")?,
                    buffers_clean: row.try_get("buffers_clean")?,
                    maxwritten_clean: row.try_get("maxwritten_clean")?,
                    buffers_backend: row.try_get("buffers_backend")?,
                    buffers_backend_fsync: row.try_get("buffers_backend_fsync")?,
                    buffers_alloc: row.try_get("buffers_alloc")?,
                    stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
                }),
                BgwriterVersion::V2 => BgwriterSnapshot::V2(PgStatBgwriterV2 {
                    ts: Ts(row.try_get("ts_us")?),
                    buffers_clean: row.try_get("buffers_clean")?,
                    maxwritten_clean: row.try_get("maxwritten_clean")?,
                    buffers_alloc: row.try_get("buffers_alloc")?,
                    stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
                }),
            })
        },
    )
    .await
}

#[cfg(test)]
mod tests;
