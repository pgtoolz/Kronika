//! Read each extension family once, falling back only before any batch is admitted.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::Session;
use crate::extension::ExtensionSchema;
use crate::query::{self, BatchError, BatchWrite, QueryStats};
use crate::settings::SettingsRow;
use crate::statements::{self, StatementsCapability};
use crate::statements_info;
use crate::store_plans::{self, Flavour, StorePlansCapability};
use crate::store_plans_info;

use super::capabilities::{
    DatabaseCapabilities, ExtensionKind, selected_statements, selected_statements_info,
    selected_store_plans, selected_store_plans_info,
};
use super::execution::{
    QUERY_TIMEOUT, QueryCompletion, QueryFailure, deliver, finish_batched_kind, finish_failed,
    session_for_generation,
};
use super::measurement::{QueryMeasurement, measure};
use super::probe::GenerationProbe;
use super::{PgBatch, PgCollector, PgObservation};

/// The discovered interface to use on the selected database generation.
enum ExtensionQuery {
    Statements(StatementsCapability),
    StatementsInfo(ExtensionSchema),
    StorePlans(StorePlansCapability),
    StorePlansInfo(ExtensionSchema),
}

impl ExtensionQuery {
    const fn name(&self) -> &'static str {
        match self {
            Self::Statements(_) => "pg_stat_statements",
            Self::StatementsInfo(_) => "pg_stat_statements_info",
            Self::StorePlans(_) => "pg_store_plans",
            Self::StorePlansInfo(_) => "pg_store_plans_info",
        }
    }
}

impl ExtensionKind {
    fn select(
        self,
        capabilities: &BTreeMap<String, DatabaseCapabilities>,
        current: Option<&str>,
        excluded: &BTreeSet<String>,
    ) -> Option<(String, u64, ExtensionQuery)> {
        match self {
            Self::Statements => selected_statements(capabilities, current, excluded).map(
                |(db, generation, capability)| {
                    (db, generation, ExtensionQuery::Statements(capability))
                },
            ),
            Self::StatementsInfo => selected_statements_info(capabilities, current, excluded).map(
                |(db, generation, schema)| (db, generation, ExtensionQuery::StatementsInfo(schema)),
            ),
            Self::StorePlans => selected_store_plans(capabilities, current, excluded).map(
                |(db, generation, capability)| {
                    (db, generation, ExtensionQuery::StorePlans(capability))
                },
            ),
            Self::StorePlansInfo => selected_store_plans_info(capabilities, current, excluded).map(
                |(db, generation, schema)| (db, generation, ExtensionQuery::StorePlansInfo(schema)),
            ),
        }
    }
}

impl PgCollector {
    pub(super) async fn collect_extensions<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let mut unavailable_databases = self
            .refresh_secondary_capabilities(probe.major, observe)
            .await;
        for kind in [
            ExtensionKind::Statements,
            ExtensionKind::StatementsInfo,
            ExtensionKind::StorePlans,
            ExtensionKind::StorePlansInfo,
        ] {
            let mut attempted = unavailable_databases.clone();
            while let Some((database, generation, query)) = kind.select(
                &self.capabilities,
                self.server_database.as_deref(),
                &attempted,
            ) {
                let is_current = self.server_database.as_deref() == Some(&database);
                let (completion, admitted) = self
                    .read_extension(&database, generation, query, observe, settings, admit)
                    .await?;
                match completion {
                    QueryCompletion::Complete => break,
                    QueryCompletion::CapabilityChanged => {
                        self.invalidate_capability(&database, kind);
                    }
                    QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
                        // A lost primary ends this cycle even if it already wrote rows.
                        if is_current {
                            self.clear_primary_connection();
                            return Ok(false);
                        }
                        if let Some(pool) = self.database_pool_mut(&database) {
                            pool.close();
                        }
                        self.capabilities.remove(&database);
                        self.last_discovery = None;
                        unavailable_databases.insert(database.clone());
                    }
                    QueryCompletion::ServerTimedOut | QueryCompletion::SourceFailed => {}
                }
                if !try_another_database(completion, admitted) {
                    break;
                }
                attempted.insert(database);
            }
        }
        Ok(true)
    }

    async fn read_extension<E>(
        &mut self,
        database: &str,
        generation: u64,
        query: ExtensionQuery,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(QueryCompletion, bool), E> {
        let Some(pool) = self.database_pool_mut(database) else {
            return Ok((QueryCompletion::ConnectionFailed, false));
        };
        let connection = pool.connection_label(0);
        let session = match session_for_generation(pool, generation, observe) {
            Ok(session) => session,
            Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                pool.close();
                return Ok((QueryCompletion::ConnectionFailed, false));
            }
            Err(QueryFailure::ServerTimeout) => {
                return Ok((QueryCompletion::ServerTimedOut, false));
            }
            Err(QueryFailure::Source) => return Ok((QueryCompletion::SourceFailed, false)),
        };
        let mut measured = measure(observe, query.name(), &connection, database);
        let mut admitted = false;
        let result = match query {
            ExtensionQuery::Statements(capability) => {
                statements::collect_statements(
                    session,
                    &capability,
                    measured.stats_mut(),
                    |batch| {
                        let write = admit(
                            PgBatch::Statements(capability.version, batch.rows),
                            settings.cloned(),
                        )?;
                        admitted = true;
                        Ok(write)
                    },
                )
                .await
            }
            ExtensionQuery::StorePlans(capability) => {
                read_store_plans(
                    session,
                    &capability,
                    measured.stats_mut(),
                    settings,
                    admit,
                    &mut admitted,
                )
                .await
            }
            ExtensionQuery::StatementsInfo(schema) => {
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    statements_info::collect(
                        session,
                        &schema.qualify("pg_stat_statements_info"),
                        measured.stats_mut(),
                    ),
                )
                .await
                .map(|result| result.map(PgBatch::StatementsInfo));
                return finish_info(measured, result, settings, admit);
            }
            ExtensionQuery::StorePlansInfo(schema) => {
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    store_plans_info::collect(
                        session,
                        &schema.qualify("pg_store_plans_info"),
                        measured.stats_mut(),
                    ),
                )
                .await
                .map(|result| result.map(PgBatch::StorePlansInfo));
                return finish_info(measured, result, settings, admit);
            }
        };
        Ok((finish_batched_kind(pool, measured, result)?, admitted))
    }
}

async fn read_store_plans<E>(
    session: Session<'_>,
    capability: &StorePlansCapability,
    stats: &mut QueryStats,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    admitted: &mut bool,
) -> Result<(), BatchError<E>> {
    macro_rules! collect {
        ($collect:path, $batch:ident) => {
            $collect(session, capability, stats, |batch| {
                let write = admit(PgBatch::$batch(batch.rows), settings.cloned())?;
                *admitted = true;
                Ok(write)
            })
            .await
        };
    }
    match capability.flavour {
        Flavour::OsscCompatible => collect!(store_plans::collect_ossc, StorePlansOssc),
        Flavour::Datasentinel => {
            collect!(store_plans::collect_datasentinel, StorePlansDatasentinel)
        }
        Flavour::Vadv => collect!(store_plans::collect_vadv, StorePlansVadv),
    }
}

fn finish_info<E>(
    measured: QueryMeasurement<'_>,
    result: Result<anyhow::Result<PgBatch>, tokio::time::error::Elapsed>,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<(QueryCompletion, bool), E> {
    match result {
        Ok(Ok(batch)) => {
            deliver(measured, admit, batch, settings.cloned())?.success();
            Ok((QueryCompletion::Complete, true))
        }
        other => Ok((finish_failed(measured, other), false)),
    }
}

pub(super) const fn try_another_database(completion: QueryCompletion, admitted: bool) -> bool {
    !admitted
        && !matches!(
            completion,
            QueryCompletion::Complete | QueryCompletion::ServerTimedOut
        )
}
