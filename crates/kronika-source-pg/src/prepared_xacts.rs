//! `pg_prepared_xacts` per-database aggregate collection for type `1_010_001`.
//!
//! Two-phase-commit transactions (`PREPARE TRANSACTION`) awaiting resolution,
//! summarized per database: how many are prepared, the oldest wall-clock age,
//! and the highest XID age. The view is cluster-wide and tags each transaction
//! with its database; grouping by database keeps the database that a forgotten
//! 2PC blocks vacuum in. Returns no rows when nothing is prepared (the default,
//! since `max_prepared_transactions` is 0). The caller interns `datname`.

use kronika_registry::pg_prepared_xacts::PgPreparedXacts;
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};

/// One raw per-database `pg_prepared_xacts` aggregate row.
///
/// `datname` is owned here and interned by the caller; numbers are owned
/// directly. See [`PgPreparedXacts`] for meaning.
#[derive(Debug, Clone)]
pub struct PreparedXactsRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Database holding these prepared transactions.
    pub datname: String,
    /// Prepared transactions in this database.
    pub prepared_count: i64,
    /// Age of the oldest prepared transaction in this database, microseconds.
    pub max_age_us: i64,
    /// Highest transaction-id age in this database.
    pub max_xid_age_tx: i64,
}

/// Build the typed `1_010_001` row, interning `datname`.
///
/// # Errors
/// Returns the interner's error if `datname` cannot be interned.
pub fn to_prepared_xacts<E>(
    row: &PreparedXactsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgPreparedXacts, E> {
    Ok(PgPreparedXacts {
        ts: Ts(row.ts),
        datname: intern(row.datname.as_bytes())?,
        prepared_count: row.prepared_count,
        max_age_us: row.max_age_us,
        max_xid_age_tx: row.max_xid_age_tx,
    })
}

/// Collect the per-database `pg_prepared_xacts` aggregate.
///
/// Groups by database, so each row names the database holding the prepared
/// transactions; `min(prepared)` within a group is never `NULL` (the group
/// exists only because it has at least one prepared transaction). `ts` is the
/// query's `clock_timestamp()`, shared with the wall-clock age calculation.
///
/// # Errors
/// Returns the `PostgreSQL` stream error or the batch sink error.
pub async fn collect_prepared_xacts<E>(
    session: Session<'_>,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<PreparedXactsRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    query::read_batched(
        session,
        marked!(
            "WITH snap AS (SELECT clock_timestamp() AS ts) \
             SELECT database::text AS datname, \
             count(*)::int8 AS prepared_count, \
             greatest(0::int8, (extract(epoch from (snap.ts - min(prepared))) * 1e6)::int8) \
             AS max_age_us, \
             max(age(transaction))::int8 AS max_xid_age_tx, \
             (extract(epoch from snap.ts) * 1e6)::int8 AS ts_us \
             FROM pg_prepared_xacts, snap GROUP BY database, snap.ts"
        ),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(PreparedXactsRow {
                ts: row.try_get("ts_us")?,
                datname: row.try_get("datname")?,
                prepared_count: row.try_get("prepared_count")?,
                max_age_us: row.try_get("max_age_us")?,
                max_xid_age_tx: row.try_get("max_xid_age_tx")?,
            })
        },
        |_row| 0,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{PreparedXactsRow, to_prepared_xacts};
    use crate::test_intern as fake_intern;

    #[test]
    fn maps_every_field_and_interns_datname() {
        let r = PreparedXactsRow {
            ts: 2_000,
            datname: "appdb".to_owned(),
            prepared_count: 3,
            max_age_us: 4_200_000,
            max_xid_age_tx: 88,
        };
        let typed = to_prepared_xacts(&r, fake_intern).expect("infallible intern");
        assert_eq!(typed.ts.0, 2_000);
        assert_eq!(typed.prepared_count, 3);
        assert_eq!(typed.max_age_us, 4_200_000);
        assert_eq!(typed.max_xid_age_tx, 88);
        assert_eq!(typed.datname, fake_intern(b"appdb").unwrap());
    }

    #[test]
    fn intern_failure_propagates() {
        let r = PreparedXactsRow {
            ts: 1,
            datname: "db".to_owned(),
            prepared_count: 1,
            max_age_us: 1,
            max_xid_age_tx: 1,
        };
        assert_eq!(to_prepared_xacts(&r, |_| Err("full")), Err("full"));
    }
}
