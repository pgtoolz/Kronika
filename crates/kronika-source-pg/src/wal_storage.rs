//! Current `pg_wal` regular-file size for type `1_020_001`.

use kronika_registry::{PgWalStorage, Ts};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

const QUERY: &str = marked!(
    "SELECT (extract(epoch FROM pg_catalog.statement_timestamp()) * 1e6)::int8 AS ts_us, \
     COALESCE(pg_catalog.sum(w.size), 0::numeric)::int8 AS wal_files_bytes \
     FROM pg_catalog.pg_ls_waldir() AS w"
);

/// Return the query for audited `PostgreSQL` majors 10 through 18.
#[must_use]
pub const fn wal_storage_query(major: u32) -> Option<&'static str> {
    if major >= 10 && major <= 18 {
        Some(QUERY)
    } else {
        None
    }
}

/// Collect one exact aggregate row, or no source outside supported versions.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol, permission, overflow, or row-decoding errors.
pub async fn collect(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
) -> anyhow::Result<Option<PgWalStorage>> {
    let Some(sql) = wal_storage_query(major) else {
        return Ok(None);
    };
    query::read_one(
        session,
        sql,
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(PgWalStorage {
                ts: Ts(row.try_get("ts_us")?),
                wal_files_bytes: row.try_get("wal_files_bytes")?,
            })
        },
    )
    .await
    .map(Some)
}

#[cfg(test)]
mod tests;
