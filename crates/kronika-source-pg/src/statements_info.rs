//! `pg_stat_statements_info` collection for type `1_015_001`.

use kronika_registry::Ts;
use kronika_registry::pg_stat_statements_info::PgStatStatementsInfo;
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

const MARKER: &str = marked!();

/// Build the one-row query for a safely qualified view supplied by discovery.
#[must_use]
pub fn query(qualified_view: &str) -> String {
    format!(
        "{MARKER}SELECT dealloc, \
         (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
         (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
         FROM {qualified_view}"
    )
}

/// Collect the singleton module-level statistics row with an unnamed statement.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect(
    session: Session<'_>,
    qualified_view: &str,
    stats: &mut QueryStats,
) -> anyhow::Result<PgStatStatementsInfo> {
    query::read_one(
        session,
        &query(qualified_view),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| {
            Ok(PgStatStatementsInfo {
                ts: Ts(row.try_get("ts_us")?),
                dealloc: row.try_get("dealloc")?,
                stats_reset: Ts(row.try_get("stats_reset_us")?),
            })
        },
    )
    .await
}

#[cfg(test)]
mod tests;
