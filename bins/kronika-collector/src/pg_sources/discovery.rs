//! Refresh connectable databases and the extension readers available in each one.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use kronika_source_pg::{Pool, databases, extension, query};

use super::capabilities::{
    DatabaseCapabilities, capabilities_from_inventory, capabilities_match_generation,
    warn_outdated_statements_layout,
};
use super::execution::{
    QUERY_TIMEOUT, QueryFailure, finish_query, open_session, session_for_generation,
};
use super::measurement::measure;
use super::probe::GenerationProbe;
use super::{PgObservation, PgSources};

/// Refresh databases, extension layouts, and role visibility every five minutes.
/// Forced collection also refreshes immediately.
pub(super) const DISCOVERY_INTERVAL: Duration = Duration::from_mins(5);

pub(super) fn discovery_due(last: Option<Instant>, now: Instant, forced: bool) -> bool {
    forced || last.is_none_or(|last| now.saturating_duration_since(last) >= DISCOVERY_INTERVAL)
}

impl PgSources {
    pub(super) async fn discover(
        &mut self,
        probe: &GenerationProbe,
        now: Instant,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> bool {
        self.last_discovery = Some(now);
        let Some(server) = self.server.as_mut() else {
            return false;
        };
        let found = match enumerate_databases(server, probe, observe).await {
            Ok(found) => found,
            Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                self.clear_primary_connection();
                return false;
            }
            Err(QueryFailure::ServerTimeout) => {
                self.last_discovery = None;
                return true;
            }
            Err(QueryFailure::Source) => {
                // A role may read local statistics without permission to enumerate databases.
                self.last_discovery = None;
                vec![databases::Database {
                    oid: probe.datid,
                    name: probe.database.clone(),
                    is_current: true,
                }]
            }
        };
        self.server_database = found
            .iter()
            .find(|database| database.is_current)
            .map(|database| database.name.clone());
        if let Some(server) = self.server.as_ref() {
            databases::refresh(&mut self.databases, &found, server);
        }
        if !self.discover_capabilities(&found, probe, observe).await {
            return false;
        }
        self.discovered = found;
        true
    }

    async fn discover_capabilities(
        &mut self,
        databases: &[databases::Database],
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> bool {
        let previous = self.capabilities.clone();
        let mut capabilities = BTreeMap::new();
        for database in databases {
            let Some(pool) = self.database_pool_mut(&database.name) else {
                continue;
            };
            let expected = database.is_current.then_some(probe.generation);
            match read_inventory(pool, database, probe.major, expected, observe).await {
                Ok(found) => {
                    capabilities.insert(database.name.clone(), found);
                }
                Err(InventoryFailure { generation, kind }) => match kind {
                    QueryFailure::Timeout | QueryFailure::Connection => {
                        pool.close();
                        if database.is_current {
                            self.clear_primary_connection();
                            return false;
                        }
                    }
                    QueryFailure::ServerTimeout => {
                        // A server-side cancellation keeps this connection usable. Keep
                        // its prior inventory only if it belongs to the same generation.
                        let generation = generation.or_else(|| pool.generation());
                        self.last_discovery = None;
                        if let Some(cached) = previous
                            .get(&database.name)
                            .filter(|cached| Some(cached.generation) == generation)
                        {
                            capabilities.insert(database.name.clone(), cached.clone());
                        }
                    }
                    QueryFailure::Source => {
                        if let Some(generation) = generation {
                            capabilities.insert(
                                database.name.clone(),
                                DatabaseCapabilities {
                                    generation,
                                    ..DatabaseCapabilities::default()
                                },
                            );
                        }
                    }
                },
            }
        }
        self.capabilities = capabilities;
        true
    }

    pub(super) async fn refresh_secondary_capabilities(
        &mut self,
        server_major: u32,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> BTreeSet<String> {
        let mut unavailable = BTreeSet::new();
        for database in self.discovered.clone() {
            if self.server_database.as_deref() == Some(database.name.as_str()) {
                continue;
            }
            let cached = self
                .capabilities
                .get(&database.name)
                .map(|entry| entry.generation);
            let Some(pool) = self.databases.get_mut(&database.name) else {
                continue;
            };
            if capabilities_match_generation(cached, pool.generation()) {
                continue;
            }
            match read_inventory(pool, &database, server_major, None, observe).await {
                Ok(found) => {
                    self.capabilities.insert(database.name, found);
                }
                Err(InventoryFailure { generation, kind }) => match kind {
                    QueryFailure::Timeout | QueryFailure::Connection => {
                        pool.close();
                        unavailable.insert(database.name.clone());
                        self.capabilities.remove(&database.name);
                    }
                    QueryFailure::ServerTimeout => {
                        // Do not select a stale inventory for this cycle, even though
                        // cancellation leaves the connection usable.
                        unavailable.insert(database.name.clone());
                    }
                    QueryFailure::Source => {
                        if let Some(generation) = generation {
                            self.capabilities.insert(
                                database.name,
                                DatabaseCapabilities {
                                    generation,
                                    ..DatabaseCapabilities::default()
                                },
                            );
                        } else {
                            self.capabilities.remove(&database.name);
                        }
                    }
                },
            }
        }
        unavailable
    }
}

async fn enumerate_databases(
    pool: &mut Pool,
    probe: &GenerationProbe,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<Vec<databases::Database>, QueryFailure> {
    let connection = pool.connection_label(0);
    let session = session_for_generation(pool, probe.generation, observe)?;
    let mut measured = measure(observe, "databases", &connection, &probe.database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        databases::enumerate(session, measured.stats_mut()),
    )
    .await;
    finish_query(measured, result)
}

struct InventoryFailure {
    /// Present only after a session was acquired and its inventory query ran.
    /// A failed query can cache an empty inventory; a failed connection cannot.
    generation: Option<u64>,
    kind: QueryFailure,
}

async fn read_inventory(
    pool: &mut Pool,
    database: &databases::Database,
    major: u32,
    expected_generation: Option<u64>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<DatabaseCapabilities, InventoryFailure> {
    let connection = pool.connection_label(0);
    let session = match expected_generation {
        Some(generation) => session_for_generation(pool, generation, observe),
        None => open_session(pool, observe).await,
    }
    .map_err(|kind| InventoryFailure {
        generation: None,
        kind,
    })?;
    let generation = session.generation();
    let mut measured = measure(observe, "extension_inventory", &connection, &database.name);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        extension::inventory(session, measured.stats_mut()),
    )
    .await;
    let inventory = finish_query(measured, result).map_err(|kind| InventoryFailure {
        generation: Some(generation),
        kind,
    })?;
    warn_outdated_statements_layout(&inventory, major, &database.name);
    Ok(capabilities_from_inventory(&inventory, generation, major))
}
