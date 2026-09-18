//! `pg_stat_archiver` collection for type `1_008_001`.
//!
//! One row; the caller interns WAL file names.

use kronika_registry::pg_stat_archiver::PgStatArchiver;
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::query::{self, QueryStats};
use crate::{Session, intern_opt as opt};

const QUERY: &str = marked!(
    "SELECT archived_count, last_archived_wal, \
     (extract(epoch from last_archived_time) * 1e6)::int8 AS last_archived_time_us, \
     failed_count, last_failed_wal, \
     (extract(epoch from last_failed_time) * 1e6)::int8 AS last_failed_time_us, \
     (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
     (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
     FROM pg_stat_archiver"
);

/// Collected row before WAL-name interning.
#[derive(Debug, Clone)]
pub struct ArchiverRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// WAL files archived.
    pub archived_count: i64,
    /// Last archived WAL file name.
    pub last_archived_wal: Option<String>,
    /// Last archive time, unix microseconds.
    pub last_archived_time: Option<i64>,
    /// Failed archive attempts.
    pub failed_count: i64,
    /// WAL file of the last failed attempt.
    pub last_failed_wal: Option<String>,
    /// Last failed attempt time, unix microseconds.
    pub last_failed_time: Option<i64>,
    /// Last reset, unix microseconds.
    pub stats_reset: Option<i64>,
}

/// Build the typed `1_008_001` row, interning WAL file names.
///
/// # Errors
/// Returns the interner's error if a WAL name cannot be interned.
pub fn to_archiver<E>(
    row: &ArchiverRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatArchiver, E> {
    Ok(PgStatArchiver {
        ts: Ts(row.ts),
        archived_count: row.archived_count,
        last_archived_wal: opt(&mut intern, row.last_archived_wal.as_deref())?,
        last_archived_time: row.last_archived_time.map(Ts),
        failed_count: row.failed_count,
        last_failed_wal: opt(&mut intern, row.last_failed_wal.as_deref())?,
        last_failed_time: row.last_failed_time.map(Ts),
        stats_reset: row.stats_reset.map(Ts),
    })
}

/// Collect the single `pg_stat_archiver` row (present on every PG 10-18).
///
/// # Errors
/// Returns `PostgreSQL` query errors.
pub async fn collect_archiver(
    session: Session<'_>,
    stats: &mut QueryStats,
) -> anyhow::Result<ArchiverRow> {
    query::read_one(
        session,
        QUERY,
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(ArchiverRow {
                ts: row.try_get("ts_us")?,
                archived_count: row.try_get("archived_count")?,
                last_archived_wal: row.try_get("last_archived_wal")?,
                last_archived_time: row.try_get("last_archived_time_us")?,
                failed_count: row.try_get("failed_count")?,
                last_failed_wal: row.try_get("last_failed_wal")?,
                last_failed_time: row.try_get("last_failed_time_us")?,
                stats_reset: row.try_get("stats_reset_us")?,
            })
        },
    )
    .await
}

#[cfg(test)]
#[path = "tests/archiver.rs"]
mod tests;
