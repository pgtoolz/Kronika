//! Configure `PostgreSQL` acquisition and adapt its batches to collector storage.

mod buffering;
pub(crate) mod query_diagnostics;

use crate::config::Config;
use crate::scheduler::{DueSet, SourceKind};
use kronika_source_pg::{PgCollectionSelection, PgCollector, Pool, Transport};

pub(crate) use buffering::{push_pg_batch, push_settings as push_pg_settings};
pub(crate) use kronika_source_pg::{
    ConnectionObservation, PgBatch, PgObservation, QueryObservation, QueryOutcome,
};

/// Convert collector configuration into explicit library connection inputs.
pub(crate) fn open(config: &Config) -> anyhow::Result<PgCollector> {
    let Some(dsn) = config.pg_dsn.as_deref() else {
        return Ok(PgCollector::default());
    };
    let transport = Transport::from_ca_file(config.pg_ssl_root_cert.as_deref())?;
    let pool = Pool::with_transport(dsn, transport)
        .map_err(|_error| anyhow::anyhow!("KRONIKA_PG_DSN is not a valid connection string"))?;
    Ok(PgCollector::new(pool))
}

/// Translate application cadence without exposing the scheduler to source-pg.
pub(crate) fn collection_selection(due: &DueSet) -> PgCollectionSelection {
    PgCollectionSelection {
        activity: due.has(SourceKind::PgActivity),
        instance: due.has(SourceKind::PgInstance),
        statements_and_plans: due.has(SourceKind::PgStatementsAndPlans),
        tables_and_indexes: due.has(SourceKind::PgTablesAndIndexes),
        refresh_discovery: due.forced(),
    }
}

#[cfg(test)]
#[path = "tests/pg_sources/mod.rs"]
mod tests;
