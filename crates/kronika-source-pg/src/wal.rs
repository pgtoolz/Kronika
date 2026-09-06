//! `pg_stat_wal` collection for types `1_007_001` / `1_007_002`.
//!
//! PG14+ cluster-wide WAL counters. PG18 uses the V2 layout after write/sync
//! fields moved to `pg_stat_io`.

use kronika_registry::Ts;
use kronika_registry::pg_stat_wal::{PgStatWalV1, PgStatWalV2};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

/// `pg_stat_wal` layout for a server major.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalVersion {
    /// PG 14-17: type `1_007_001` (with write/sync counters and timings).
    V1,
    /// PG 18: type `1_007_002` (generation counters only).
    V2,
}

/// Return the WAL layout, or `None` before PG14.
#[must_use]
pub const fn wal_version(major: u32) -> Option<WalVersion> {
    if major >= 18 {
        Some(WalVersion::V2)
    } else if major >= 14 {
        Some(WalVersion::V1)
    } else {
        None
    }
}

/// The SQL for one layout.
///
/// `ts` is one `statement_timestamp()`. `wal_bytes` is `numeric` in
/// `PostgreSQL`; casting to `int8` makes overflow fail collection instead of
/// truncating.
#[must_use]
pub const fn wal_query(version: WalVersion) -> &'static str {
    match version {
        WalVersion::V1 => marked!(
            "SELECT wal_records, wal_fpi, wal_bytes::int8 AS wal_bytes, wal_buffers_full, \
             wal_write, wal_sync, wal_write_time, wal_sync_time, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_wal"
        ),
        WalVersion::V2 => marked!(
            "SELECT wal_records, wal_fpi, wal_bytes::int8 AS wal_bytes, wal_buffers_full, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_wal"
        ),
    }
}

/// One `pg_stat_wal` snapshot in the version's typed layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WalSnapshot {
    /// PG 14-17 layout.
    V1(PgStatWalV1),
    /// PG 18 layout.
    V2(PgStatWalV2),
}

fn v1_from_pg(row: &tokio_postgres::Row) -> anyhow::Result<PgStatWalV1> {
    Ok(PgStatWalV1 {
        ts: Ts(row.try_get("ts_us")?),
        wal_records: row.try_get("wal_records")?,
        wal_fpi: row.try_get("wal_fpi")?,
        wal_bytes: row.try_get("wal_bytes")?,
        wal_buffers_full: row.try_get("wal_buffers_full")?,
        wal_write: row.try_get("wal_write")?,
        wal_sync: row.try_get("wal_sync")?,
        wal_write_time: row.try_get("wal_write_time")?,
        wal_sync_time: row.try_get("wal_sync_time")?,
        stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
    })
}

fn v2_from_pg(row: &tokio_postgres::Row) -> anyhow::Result<PgStatWalV2> {
    Ok(PgStatWalV2 {
        ts: Ts(row.try_get("ts_us")?),
        wal_records: row.try_get("wal_records")?,
        wal_fpi: row.try_get("wal_fpi")?,
        wal_bytes: row.try_get("wal_bytes")?,
        wal_buffers_full: row.try_get("wal_buffers_full")?,
        stats_reset: row.try_get::<_, Option<i64>>("stats_reset_us")?.map(Ts),
    })
}

/// Collect the single `pg_stat_wal` row, or `None` before PG14 where the view
/// does not exist.
///
/// # Errors
/// Returns `PostgreSQL` query errors.
pub async fn collect_wal(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
) -> anyhow::Result<Option<WalSnapshot>> {
    let Some(version) = wal_version(major) else {
        return Ok(None);
    };
    query::read_one(
        session,
        wal_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| match version {
            WalVersion::V1 => Ok(WalSnapshot::V1(v1_from_pg(row)?)),
            WalVersion::V2 => Ok(WalSnapshot::V2(v2_from_pg(row)?)),
        },
    )
    .await
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::{WalVersion, wal_query, wal_version};

    #[test]
    fn version_appears_at_pg14_and_changes_at_pg18() {
        assert_eq!(wal_version(10), None);
        assert_eq!(wal_version(13), None);
        assert_eq!(wal_version(14), Some(WalVersion::V1));
        assert_eq!(wal_version(17), Some(WalVersion::V1));
        assert_eq!(wal_version(18), Some(WalVersion::V2));
    }

    #[test]
    fn query_includes_version_specific_columns() {
        assert!(wal_query(WalVersion::V1).contains("wal_write_time"));
        assert!(wal_query(WalVersion::V1).contains("wal_sync"));
        assert!(!wal_query(WalVersion::V2).contains("wal_write"));
        assert!(!wal_query(WalVersion::V2).contains("wal_sync"));
        for version in [WalVersion::V1, WalVersion::V2] {
            assert!(wal_query(version).contains("pg_stat_wal"));
            assert!(wal_query(version).contains("kronika:"));
            assert!(wal_query(version).contains("wal_bytes::int8 AS wal_bytes"));
        }
    }
}
