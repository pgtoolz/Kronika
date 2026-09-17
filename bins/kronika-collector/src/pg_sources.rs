//! Sequential `PostgreSQL` statistics collection.
//!
//! Connections are reused across cycles, with queries awaited one at a time.
//! Lost connections and client timeouts close the affected pool; losing the
//! primary ends the cycle. Ordinary SQL errors skip the affected source.
//! The modules below handle
//! discovery, fixed server views, extensions, and per-database relation views.

mod batch;
mod buffering;
mod capabilities;
mod discovery;
mod execution;
mod extensions;
mod instance;
mod measurement;
mod probe;
pub(crate) mod query_diagnostics;
mod relations;
mod settings;

use crate::config::Config;
use crate::scheduler::{DueSet, SourceKind};
use capabilities::DatabaseCapabilities;
use discovery::discovery_due;
use kronika_source_pg::{Pool, databases, query::BatchWrite, settings::SettingsRow};
use probe::GenerationProbe;
use settings::{CachedSettings, cached_settings_for_generation};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

pub(crate) use batch::PgBatch;
pub(crate) use buffering::{push_pg_batch, push_settings as push_pg_settings};
pub(crate) use execution::postgres_query_cancelled;
pub(crate) use measurement::{
    ConnectionObservation, PgObservation, QueryObservation, QueryOutcome,
};

/// The primary connection, persistent per-database connections, and caches
/// tied to the primary connection generation.
#[derive(Debug, Default)]
pub(crate) struct PgSources {
    server: Option<Pool>,
    server_database: Option<String>,
    databases: BTreeMap<String, Pool>,
    discovered: Vec<databases::Database>,
    capabilities: BTreeMap<String, DatabaseCapabilities>,
    last_discovery: Option<Instant>,
    settings: Option<CachedSettings>,
    probe: Option<GenerationProbe>,
}

impl PgSources {
    /// Take the selected DSN, or nothing when none is configured.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        let Some(dsn) = config.pg_dsn.as_deref() else {
            return Ok(Self::default());
        };
        let transport =
            kronika_source_pg::Transport::from_ca_file(config.pg_ssl_root_cert.as_deref())?;
        let server = Pool::with_transport(dsn, transport)
            .map_err(|_error| anyhow::anyhow!("KRONIKA_PG_DSN is not a valid connection string"))?;
        Ok(Self {
            server: Some(server),
            ..Self::default()
        })
    }

    /// Configuration rows safe to attach to a newly opened segment.
    pub(crate) fn last_settings(&self) -> Option<Arc<[SettingsRow]>> {
        let generation = self.server.as_ref().and_then(Pool::generation)?;
        cached_settings_for_generation(self.settings.as_ref(), generation)
    }

    pub(crate) fn close_connections(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.close();
        }
        self.close_secondary_connections();
    }

    /// Read due sections and synchronously admit each bounded batch before the
    /// query stream fetches another row.
    pub(crate) async fn collect<E>(
        &mut self,
        due: &DueSet,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        mut admit: impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        if ![
            SourceKind::PgActivity,
            SourceKind::PgInstance,
            SourceKind::PgStatementsAndPlans,
            SourceKind::PgTablesAndIndexes,
        ]
        .into_iter()
        .any(|kind| due.has(kind))
        {
            return Ok(());
        }
        let now = Instant::now();
        let refresh_probe = discovery_due(self.last_discovery, now, due.forced());
        let Some(probe) = self.read_probe(refresh_probe, observe).await else {
            return Ok(());
        };
        if discovery_due(self.last_discovery, now, due.forced())
            && !self.discover(&probe, now, observe).await
        {
            return Ok(());
        }

        // Every group stops on a lost primary connection or a rejected WAL batch.
        macro_rules! collect_group {
            ($kind:ident, $collection:expr) => {
                if due.has(SourceKind::$kind) {
                    match $collection.await {
                        Ok(true) => {}
                        Ok(false) => {
                            self.clear_primary_connection();
                            return Ok(());
                        }
                        Err(error) => {
                            self.clear_primary_cache_if_closed();
                            return Err(error);
                        }
                    }
                }
            };
        }

        // Read transient activity before the larger, less frequent snapshots.
        let cached_settings = self.last_settings();
        collect_group!(
            PgActivity,
            self.collect_activity(&probe, observe, cached_settings.as_ref(), &mut admit)
        );
        collect_group!(
            PgInstance,
            self.collect_instance(&probe, observe, &mut admit)
        );
        let cached_settings = self.last_settings();
        collect_group!(
            PgStatementsAndPlans,
            self.collect_extensions(&probe, observe, cached_settings.as_ref(), &mut admit)
        );
        if due.has(SourceKind::PgTablesAndIndexes) {
            self.collect_relations(&probe, observe, cached_settings.as_ref(), &mut admit)
                .await?;
        }
        Ok(())
    }

    fn database_pool_mut(&mut self, database: &str) -> Option<&mut Pool> {
        if self.server_database.as_deref() == Some(database) {
            self.server.as_mut()
        } else {
            self.databases.get_mut(database)
        }
    }

    fn clear_primary_connection(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.close();
        }
        self.close_secondary_connections();
        self.settings = None;
        self.probe = None;
        self.server_database = None;
        self.capabilities.clear();
        self.discovered.clear();
        self.last_discovery = None;
    }

    fn clear_primary_cache_if_closed(&mut self) {
        if self.server.as_ref().and_then(Pool::generation).is_none() {
            self.clear_primary_connection();
        }
    }

    fn update_probe_cache(&mut self, probe: GenerationProbe, same_generation: bool) {
        if !same_generation {
            self.close_secondary_connections();
            self.settings = None;
            self.capabilities.clear();
            self.discovered.clear();
            self.last_discovery = None;
        }
        self.probe = Some(probe);
    }

    fn close_secondary_connections(&mut self) {
        for pool in self.databases.values_mut() {
            pool.close();
        }
        self.databases.clear();
    }
}

#[cfg(test)]
#[path = "tests/pg_sources/mod.rs"]
mod tests;
