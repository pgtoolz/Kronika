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
mod tests {
    use super::{ArchiverRow, to_archiver};
    use crate::test_intern as fake_intern;

    #[derive(Clone, Copy)]
    enum WalSamples {
        Both,
        Neither,
        FailedOnly,
    }

    fn sample_row(samples: WalSamples) -> ArchiverRow {
        let (archived, failed) = match samples {
            WalSamples::Both => (true, true),
            WalSamples::Neither => (false, false),
            WalSamples::FailedOnly => (false, true),
        };
        ArchiverRow {
            ts: 2_000,
            archived_count: 100,
            last_archived_wal: archived.then(|| "000000010000000000000005".to_owned()),
            last_archived_time: archived.then_some(1_500),
            failed_count: 2,
            last_failed_wal: failed.then(|| "000000010000000000000006".to_owned()),
            last_failed_time: failed.then_some(1_750),
            stats_reset: Some(1_000),
        }
    }

    #[test]
    fn interns_wal_names_and_maps_times() {
        let r = to_archiver(&sample_row(WalSamples::Both), fake_intern).expect("intern");
        assert_eq!(r.ts.0, 2_000);
        assert_eq!(r.archived_count, 100);
        assert_eq!(
            r.last_archived_wal,
            Some(fake_intern(b"000000010000000000000005").unwrap())
        );
        assert_eq!(r.last_archived_time.map(|t| t.0), Some(1_500));
        assert_eq!(
            r.last_failed_wal,
            Some(fake_intern(b"000000010000000000000006").unwrap())
        );
        assert_eq!(r.last_failed_time.map(|t| t.0), Some(1_750));
    }

    #[test]
    fn handles_null_wal_names() {
        let r = to_archiver(&sample_row(WalSamples::Neither), fake_intern).expect("intern");
        assert_eq!(r.last_archived_wal, None);
        assert_eq!(r.last_archived_time, None);
        assert_eq!(r.last_failed_wal, None);
        assert_eq!(r.last_failed_time, None);
    }

    #[test]
    fn intern_failure_propagates() {
        assert_eq!(
            to_archiver(&sample_row(WalSamples::FailedOnly), |_| Err("full")),
            Err("full")
        );
    }
}
