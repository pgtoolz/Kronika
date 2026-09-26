//! Sequential `PostgreSQL` statistics collection.
//!
//! Connections are reused across cycles, with queries awaited one at a time.
//! Lost connections and client timeouts close the affected pool; losing the
//! primary ends the cycle. Ordinary SQL errors skip the affected source.
//! The modules below handle
//! discovery, fixed server views, extensions, and per-database relation views.

mod batch;
mod capabilities;
mod discovery;
pub(crate) mod execution;
mod extensions;
mod instance;
mod measurement;
mod probe;
mod relations;
mod settings;

use crate::{Pool, databases, query::BatchWrite, settings::SettingsRow};
use capabilities::DatabaseCapabilities;
use discovery::discovery_due;
use probe::GenerationProbe;
use settings::{CachedSettings, cached_settings_for_generation};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

pub use batch::PgBatch;
pub use measurement::{
    ConnectionObservation, PgObservation, PgWarning, QueryObservation, QueryOutcome,
};

/// Source groups selected for one acquisition pass. Scheduling remains caller-owned.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent source groups can be selected in any combination"
)]
pub struct PgCollectionSelection {
    /// Read activity, locks and vacuum progress.
    pub activity: bool,
    /// Read settings and fixed server counters.
    pub instance: bool,
    /// Read statement and plan extensions.
    pub statements_and_plans: bool,
    /// Read per-database relation statistics.
    pub tables_and_indexes: bool,
    /// Refresh database and capability discovery before its normal deadline.
    pub refresh_discovery: bool,
}

impl PgCollectionSelection {
    const fn any(self) -> bool {
        self.activity || self.instance || self.statements_and_plans || self.tables_and_indexes
    }
}

/// The primary connection, persistent per-database connections, and caches
/// tied to the primary connection generation.
#[derive(Debug, Default)]
pub struct PgCollector {
    server: Option<Pool>,
    server_database: Option<String>,
    databases: BTreeMap<String, Pool>,
    discovered: Vec<databases::Database>,
    capabilities: BTreeMap<String, DatabaseCapabilities>,
    last_discovery: Option<Instant>,
    discovery_generation: u64,
    settings: Option<CachedSettings>,
    probe: Option<GenerationProbe>,
}

impl PgCollector {
    /// Use one explicitly configured, lazily opened primary connection.
    #[must_use]
    pub fn new(server: Pool) -> Self {
        Self {
            server: Some(server),
            ..Self::default()
        }
    }

    /// Configuration rows safe to attach to a newly opened segment.
    #[must_use]
    pub fn last_settings(&self) -> Option<Arc<[SettingsRow]>> {
        let generation = self.server.as_ref().and_then(Pool::generation)?;
        cached_settings_for_generation(self.settings.as_ref(), generation)
    }

    /// Close primary and secondary sessions; acquisition reconnects when needed.
    pub fn close_connections(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.close();
        }
        self.close_secondary_connections();
    }

    /// Names from the last database discovery, empty before the first pass.
    ///
    /// The Prometheus exporter reuses this list instead of running its own
    /// discovery; it refreshes on the collector's normal discovery cadence.
    #[must_use]
    pub fn discovered_database_names(&self) -> Vec<String> {
        self.discovered.iter().map(|db| db.name.clone()).collect()
    }

    /// Counter bumped every time database discovery reruns.
    ///
    /// Callers compare it between passes to notice a discovery cycle; the
    /// exporter uses that to re-enable metrics disabled for missing objects.
    #[must_use]
    pub const fn discovery_generation(&self) -> u64 {
        self.discovery_generation
    }

    /// Read due sections and synchronously admit each bounded batch before the
    /// query stream fetches another row.
    ///
    /// # Errors
    /// Returns the sink error immediately after closing any unfinished query stream.
    pub async fn collect<E>(
        &mut self,
        selection: &PgCollectionSelection,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        mut admit: impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        if !selection.any() {
            return Ok(());
        }
        let now = Instant::now();
        let refresh_probe = discovery_due(self.last_discovery, now, selection.refresh_discovery);
        let Some(probe) = self.read_probe(refresh_probe, observe).await else {
            return Ok(());
        };
        if discovery_due(self.last_discovery, now, selection.refresh_discovery)
            && !self.discover(&probe, now, observe).await
        {
            return Ok(());
        }

        // Every group stops on a lost primary connection or a rejected batch.
        macro_rules! collect_group {
            ($kind:ident, $collection:expr) => {
                if selection.$kind {
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
            activity,
            self.collect_activity(&probe, observe, cached_settings.as_ref(), &mut admit)
        );
        collect_group!(instance, self.collect_instance(&probe, observe, &mut admit));
        let cached_settings = self.last_settings();
        collect_group!(
            statements_and_plans,
            self.collect_extensions(&probe, observe, cached_settings.as_ref(), &mut admit)
        );
        if selection.tables_and_indexes {
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
#[path = "tests/acquisition/mod.rs"]
mod tests;
