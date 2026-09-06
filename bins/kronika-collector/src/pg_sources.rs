//! Sequential `PostgreSQL` statistics collection.
//!
//! Every database keeps one healthy connection across collection cycles. A
//! failed or timed-out connection is closed and reopened on a later cycle.
//! Queries on every connection are awaited one at a time; no pipeline or
//! concurrent query task is created.

mod buffering;
pub(crate) mod telemetry;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use kronika_registry::PgWalStorage;
use kronika_registry::pg_stat_statements_info::PgStatStatementsInfo;
use kronika_registry::pg_store_plans_info::PgStorePlansInfo;
use kronika_source_pg::activity::{self, ActivityRow, ActivityVersion};
use kronika_source_pg::archiver::ArchiverRow;
use kronika_source_pg::bgwriter::{self, BgwriterSnapshot};
use kronika_source_pg::checkpointer::{self, CheckpointerSnapshot};
use kronika_source_pg::database::{self, DatabaseRow, DatabaseVersion};
use kronika_source_pg::databases;
use kronika_source_pg::extension;
use kronika_source_pg::io::{self, IoRow, IoVersion};
use kronika_source_pg::locks::{self, LockRow, LocksVersion};
use kronika_source_pg::prepared_xacts::{self, PreparedXactsRow};
use kronika_source_pg::progress_vacuum::{self, ProgressVacuumRow};
use kronika_source_pg::query::{self, BatchError, BatchWrite};
use kronika_source_pg::settings::{self, SettingsRow};
use kronika_source_pg::statements::{self, StatementsRow, StatementsVersion};
use kronika_source_pg::statements_info;
use kronika_source_pg::store_plans::{self, DatasentinelRow, Flavour, OsscRow, VadvRow};
use kronika_source_pg::store_plans_info;
use kronika_source_pg::user_indexes::{self, UserIndexesRow, UserIndexesVersion};
use kronika_source_pg::user_tables::{self, UserTablesRow, UserTablesVersion};
use kronika_source_pg::wal::{self, WalSnapshot};
use kronika_source_pg::wal_storage;
use kronika_source_pg::{Pool, Session};

use crate::config::Config;
use crate::logging::{LogLevel, field, log_event};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::{push_pg_batch, push_settings as push_pg_settings};

const SERVER_PROBE_SQL: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " bins/kronika-collector/src/pg_sources.rs */ ",
    "SELECT current_setting('server_version_num'), d.oid::text, d.datname::text, ",
    "r.oid::text, r.rolname::text, ",
    "pg_catalog.pg_has_role('pg_read_all_stats', 'USAGE') ",
    "FROM pg_catalog.pg_database AS d CROSS JOIN pg_catalog.pg_roles AS r ",
    "WHERE d.datname = current_database() AND r.rolname = session_user"
);
const QUERY_TIMEOUT: Duration = query::QUERY_FETCH_TIMEOUT;
const DISCOVERY_INTERVAL: Duration = Duration::from_mins(5);

/// A low-cardinality measurement hook for the canonical collector telemetry.
#[derive(Debug)]
pub(crate) enum PgObservation {
    Query(QueryObservation),
    Connection(ConnectionObservation),
}

/// One completed or interrupted SQL statement.
#[derive(Debug)]
pub(crate) struct QueryObservation {
    pub(crate) query_name: &'static str,
    pub(crate) connection: String,
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) stats: query::QueryStats,
    pub(crate) outcome: QueryOutcome,
    pub(crate) error: Option<String>,
}

/// One failed connection attempt.
#[derive(Debug)]
pub(crate) struct ConnectionObservation {
    pub(crate) connection: String,
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) timeout: bool,
    pub(crate) closed: bool,
    pub(crate) error: String,
}

/// Stable query completion classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryOutcome {
    Success,
    Error,
    Timeout,
    SinkError,
}

struct QueryMeasurement<'a> {
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    connection: String,
    database: String,
    started: Instant,
    stats: query::QueryStats,
    finished: bool,
}

impl QueryMeasurement<'_> {
    const fn stats_mut(&mut self) -> &mut query::QueryStats {
        &mut self.stats
    }

    fn resolve_identity(&mut self, connection: String, database: String) {
        self.connection = connection;
        self.database = database;
    }

    fn success(mut self) {
        self.emit(QueryOutcome::Success, None);
    }

    fn error(mut self, message: String) {
        self.emit(QueryOutcome::Error, Some(message));
    }

    fn timeout(mut self) {
        self.emit(
            QueryOutcome::Timeout,
            Some(format!(
                "query timed out after {} seconds",
                QUERY_TIMEOUT.as_secs()
            )),
        );
    }

    fn server_timeout(mut self, message: String) {
        self.emit(QueryOutcome::Timeout, Some(message));
    }

    fn sink_error(mut self) {
        self.emit(
            QueryOutcome::SinkError,
            Some("write query batch to the journal failed".to_owned()),
        );
    }

    fn emit(&mut self, outcome: QueryOutcome, error: Option<String>) {
        self.finished = true;
        (self.observe)(PgObservation::Query(QueryObservation {
            query_name: self.query_name,
            connection: std::mem::take(&mut self.connection),
            database: std::mem::take(&mut self.database),
            elapsed: self.started.elapsed(),
            stats: std::mem::take(&mut self.stats),
            outcome,
            error,
        }));
    }
}

impl Drop for QueryMeasurement<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(
                QueryOutcome::Error,
                Some("collector stopped while the query was running".to_owned()),
            );
        }
    }
}

fn measure<'a>(
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    connection: &str,
    database: &str,
) -> QueryMeasurement<'a> {
    QueryMeasurement {
        observe,
        query_name,
        connection: connection.to_owned(),
        database: database.to_owned(),
        started: Instant::now(),
        stats: query::QueryStats::default(),
        finished: false,
    }
}

/// One bounded `PostgreSQL` batch retained until it reaches the WAL.
#[derive(Debug)]
pub(crate) enum PgBatch {
    Settings(Arc<[SettingsRow]>),
    Archiver(ArchiverRow),
    Bgwriter(BgwriterSnapshot),
    Checkpointer(CheckpointerSnapshot),
    Wal(WalSnapshot),
    WalStorage(PgWalStorage),
    PreparedXacts(Vec<PreparedXactsRow>),
    Database(DatabaseVersion, Vec<DatabaseRow>),
    Io(IoVersion, Vec<IoRow>),
    Activity(ActivityVersion, Vec<ActivityRow>),
    Locks(LocksVersion, Vec<LockRow>),
    ProgressVacuum(Vec<ProgressVacuumRow>),
    Statements(StatementsVersion, Vec<StatementsRow>),
    StatementsInfo(PgStatStatementsInfo),
    StorePlansOssc(Vec<OsscRow>),
    StorePlansDatasentinel(Vec<DatasentinelRow>),
    StorePlansVadv(Vec<VadvRow>),
    StorePlansInfo(PgStorePlansInfo),
    UserTables(UserTablesVersion, Vec<UserTablesRow>),
    UserIndexes(UserIndexesVersion, Vec<UserIndexesRow>),
}

#[derive(Debug)]
struct CachedSettings {
    generation: u64,
    rows: Arc<[SettingsRow]>,
}

#[derive(Debug, Clone)]
struct GenerationProbe {
    generation: u64,
    major: u32,
    datid: u32,
    database: String,
    usesysid: u32,
    user: String,
    full_visibility: bool,
}

#[derive(Debug, Clone, Default)]
struct DatabaseCapabilities {
    generation: u64,
    statements: Option<statements::StatementsCapability>,
    store_plans: Option<store_plans::StorePlansCapability>,
    statements_info: Option<extension::ExtensionSchema>,
    store_plans_info: Option<extension::ExtensionSchema>,
}

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
    /// Take the first configured DSN, or nothing when none is configured.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        let Some(dsn) = config.pg_dsns.first() else {
            return Ok(Self::default());
        };
        if config.pg_dsns.len() > 1 {
            log_event(
                LogLevel::Warn,
                "pg_metrics_single_server",
                &[
                    field("configured", config.pg_dsns.len()),
                    field(
                        "reason",
                        "a metric row does not name its server, so only the first DSN is collected",
                    ),
                ],
            );
        }
        let server = Pool::new(dsn).map_err(|_error| {
            anyhow::anyhow!("KRONIKA_PG_DSNS[0] is not a valid connection string")
        })?;
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
        let instance = due.has(SourceKind::Pg);
        let relations = due.has(SourceKind::PgRelations);
        if !instance && !relations {
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
        if instance {
            let current = self.settings.as_ref().map(|cached| cached.rows.as_ref());
            let settings_result = match self.server.as_mut() {
                Some(server) => read_settings(server, &probe, current, observe, &mut admit).await,
                None => Ok(None),
            };
            match settings_result {
                Ok(Some(rows)) => {
                    self.settings = Some(CachedSettings {
                        generation: probe.generation,
                        rows,
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        let cached_settings = self.last_settings();
        if instance {
            match self
                .collect_instance(&probe, observe, cached_settings.as_ref(), &mut admit)
                .await
            {
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
        if relations {
            self.collect_relations(&probe, observe, cached_settings.as_ref(), &mut admit)
                .await?;
        }
        Ok(())
    }

    async fn read_probe(
        &mut self,
        refresh: bool,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> Option<GenerationProbe> {
        let cached = self.probe.clone();
        let server = self.server.as_mut()?;
        let database = server.database_label().to_owned();
        let connection = server.connection_label(0);
        let session = match open_session(server, observe).await {
            Ok(session) => session,
            Err(_failure) => {
                self.clear_primary_connection();
                return None;
            }
        };
        let generation = session.generation();
        let same_generation = cached
            .as_ref()
            .is_some_and(|cached| cached.generation == generation);
        if same_generation && !refresh {
            return cached;
        }
        let mut measured = measure(observe, "server_probe", &connection, &database);
        let result = query::timeout(
            session,
            QUERY_TIMEOUT,
            read_generation_probe(session, generation, measured.stats_mut()),
        )
        .await;
        match result {
            Ok(Ok(probe)) => {
                server.remember_resolved_identity(&probe.user, &probe.database);
                measured.resolve_identity(server.connection_label(0), probe.database.clone());
                measured.success();
                self.server_database = Some(probe.database.clone());
                self.update_probe_cache(probe.clone(), same_generation);
                Some(probe)
            }
            Ok(Err(error)) => {
                let cancelled = postgres_query_cancelled(&error);
                if cancelled {
                    measured.server_timeout(format!("{error:#}"));
                } else {
                    measured.error(format!("{error:#}"));
                }
                if !cancelled && postgres_connection_error(&error) {
                    self.clear_primary_connection();
                }
                None
            }
            Err(_elapsed) => {
                measured.timeout();
                self.clear_primary_connection();
                None
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the sequential pass keeps each query and failure boundary visible"
    )]
    async fn collect_instance<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let Some(server) = self.server.as_mut() else {
            return Ok(false);
        };
        let database = probe.database.clone();
        let connection = server.connection_label(0);
        let major = probe.major;
        let generation = probe.generation;

        let archiver = {
            let session = match session_for_generation(server, generation, observe) {
                Ok(session) => session,
                Err(_failure) => return Ok(false),
            };
            let mut measured = measure(observe, "pg_stat_archiver", &connection, &database);
            let result = query::timeout(
                session,
                QUERY_TIMEOUT,
                kronika_source_pg::archiver::collect_archiver(session, measured.stats_mut()),
            )
            .await;
            match result {
                Ok(Ok(row)) => {
                    measured = deliver(
                        measured,
                        admit,
                        PgBatch::Archiver(row),
                        cached_settings.cloned(),
                    )?;
                    measured.success();
                    Some(())
                }
                other => match finish_failed(measured, other) {
                    QueryCompletion::SourceFailed
                    | QueryCompletion::CapabilityChanged
                    | QueryCompletion::ServerTimedOut => Some(()),
                    QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => None,
                    QueryCompletion::Complete => unreachable!("success handled above"),
                },
            }
        };
        if archiver.is_none() {
            self.clear_primary_connection();
            return Ok(false);
        }

        if !collect_bgwriter(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_checkpointer(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_wal_storage(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }

        if wal::wal_version(major).is_some() {
            let result = {
                let session = match session_for_generation(server, generation, observe) {
                    Ok(session) => session,
                    Err(_failure) => return Ok(false),
                };
                let mut measured = measure(observe, "pg_stat_wal", &connection, &database);
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    wal::collect_wal(session, major, measured.stats_mut()),
                )
                .await;
                match result {
                    Ok(Ok(Some(row))) => {
                        measured =
                            deliver(measured, admit, PgBatch::Wal(row), cached_settings.cloned())?;
                        measured.success();
                        true
                    }
                    Ok(Ok(None)) => {
                        measured.success();
                        true
                    }
                    other => fixed_source_can_continue(finish_failed(measured, other)),
                }
            };
            if !result {
                self.clear_primary_connection();
                return Ok(false);
            }
        }

        if !collect_prepared(
            server,
            generation,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_database(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if io::io_version(major).is_some()
            && !collect_io(
                server,
                generation,
                major,
                observe,
                &database,
                cached_settings,
                admit,
            )
            .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if probe.full_visibility {
            if !collect_activity(
                server,
                generation,
                major,
                observe,
                &database,
                cached_settings,
                admit,
            )
            .await?
                || !collect_locks(
                    server,
                    generation,
                    major,
                    observe,
                    &database,
                    cached_settings,
                    admit,
                )
                .await?
                || !collect_progress(
                    server,
                    generation,
                    major,
                    observe,
                    &database,
                    cached_settings,
                    admit,
                )
                .await?
            {
                self.clear_primary_connection();
                return Ok(false);
            }
        } else {
            log_event(
                LogLevel::Warn,
                "pg_stats_visibility_required",
                &[
                    field("database", &database),
                    field("reason", "pg_read_all_stats_required"),
                ],
            );
        }
        self.collect_extensions_dynamic(probe, observe, cached_settings, admit)
            .await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "statement and plan reads share one ordered dynamic-capability workflow"
    )]
    async fn collect_extensions_dynamic<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let mut unavailable_databases = self
            .refresh_secondary_capabilities(probe.major, observe)
            .await;
        let mut attempted = unavailable_databases.clone();
        while let Some((database, generation, capability)) = selected_statements(
            &self.capabilities,
            self.server_database.as_deref(),
            &attempted,
        ) {
            let is_current = self.server_database.as_deref() == Some(&database);
            let (completion, admitted) = 'query: {
                let Some(pool) = self.database_pool_mut(&database) else {
                    break 'query (QueryCompletion::ConnectionFailed, false);
                };
                let connection = pool.connection_label(0);
                let session = match session_for_generation(pool, generation, observe) {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        break 'query (QueryCompletion::ConnectionFailed, false);
                    }
                    Err(QueryFailure::ServerTimeout) => {
                        break 'query (QueryCompletion::ServerTimedOut, false);
                    }
                    Err(QueryFailure::Source) => {
                        break 'query (QueryCompletion::SourceFailed, false);
                    }
                };
                let version = capability.version;
                let mut measured = measure(observe, "pg_stat_statements", &connection, &database);
                let mut admitted = false;
                let result = statements::collect_statements(
                    session,
                    &capability,
                    measured.stats_mut(),
                    |batch| {
                        let write =
                            admit(PgBatch::Statements(version, batch.rows), settings.cloned())?;
                        admitted = true;
                        Ok(write)
                    },
                )
                .await;
                (finish_batched_kind(pool, measured, result)?, admitted)
            };
            match completion {
                QueryCompletion::Complete => break,
                QueryCompletion::CapabilityChanged => {
                    self.invalidate_statements(&database);
                }
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
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

        let mut attempted = unavailable_databases.clone();
        while let Some((database, generation, schema)) = selected_statements_info(
            &self.capabilities,
            self.server_database.as_deref(),
            &attempted,
        ) {
            let is_current = self.server_database.as_deref() == Some(&database);
            let (completion, admitted) = 'query: {
                let Some(pool) = self.database_pool_mut(&database) else {
                    break 'query (QueryCompletion::ConnectionFailed, false);
                };
                let connection = pool.connection_label(0);
                let session = match session_for_generation(pool, generation, observe) {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        break 'query (QueryCompletion::ConnectionFailed, false);
                    }
                    Err(QueryFailure::ServerTimeout) => {
                        break 'query (QueryCompletion::ServerTimedOut, false);
                    }
                    Err(QueryFailure::Source) => {
                        break 'query (QueryCompletion::SourceFailed, false);
                    }
                };
                let mut measured =
                    measure(observe, "pg_stat_statements_info", &connection, &database);
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    statements_info::collect(
                        session,
                        &schema.qualify("pg_stat_statements_info"),
                        measured.stats_mut(),
                    ),
                )
                .await;
                match result {
                    Ok(Ok(row)) => {
                        measured = deliver(
                            measured,
                            admit,
                            PgBatch::StatementsInfo(row),
                            settings.cloned(),
                        )?;
                        measured.success();
                        (QueryCompletion::Complete, true)
                    }
                    other => (finish_failed(measured, other), false),
                }
            };
            match completion {
                QueryCompletion::Complete => break,
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
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
                QueryCompletion::CapabilityChanged => {
                    self.invalidate_statements_info(&database);
                }
                QueryCompletion::ServerTimedOut | QueryCompletion::SourceFailed => {}
            }
            if !try_another_database(completion, admitted) {
                break;
            }
            attempted.insert(database);
        }

        let mut attempted = unavailable_databases.clone();
        while let Some((database, generation, capability)) = selected_store_plans(
            &self.capabilities,
            self.server_database.as_deref(),
            &attempted,
        ) {
            let is_current = self.server_database.as_deref() == Some(&database);
            let (completion, admitted) = 'query: {
                let Some(pool) = self.database_pool_mut(&database) else {
                    break 'query (QueryCompletion::ConnectionFailed, false);
                };
                let connection = pool.connection_label(0);
                let session = match session_for_generation(pool, generation, observe) {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        break 'query (QueryCompletion::ConnectionFailed, false);
                    }
                    Err(QueryFailure::ServerTimeout) => {
                        break 'query (QueryCompletion::ServerTimedOut, false);
                    }
                    Err(QueryFailure::Source) => {
                        break 'query (QueryCompletion::SourceFailed, false);
                    }
                };
                let mut measured = measure(observe, "pg_store_plans", &connection, &database);
                let mut admitted = false;
                let result = match capability.flavour {
                    Flavour::OsscCompatible => {
                        store_plans::collect_ossc(
                            session,
                            &capability,
                            measured.stats_mut(),
                            |batch| {
                                let write =
                                    admit(PgBatch::StorePlansOssc(batch.rows), settings.cloned())?;
                                admitted = true;
                                Ok(write)
                            },
                        )
                        .await
                    }
                    Flavour::Datasentinel => {
                        store_plans::collect_datasentinel(
                            session,
                            &capability,
                            measured.stats_mut(),
                            |batch| {
                                let write = admit(
                                    PgBatch::StorePlansDatasentinel(batch.rows),
                                    settings.cloned(),
                                )?;
                                admitted = true;
                                Ok(write)
                            },
                        )
                        .await
                    }
                    Flavour::Vadv => {
                        store_plans::collect_vadv(
                            session,
                            &capability,
                            measured.stats_mut(),
                            |batch| {
                                let write =
                                    admit(PgBatch::StorePlansVadv(batch.rows), settings.cloned())?;
                                admitted = true;
                                Ok(write)
                            },
                        )
                        .await
                    }
                };
                (finish_batched_kind(pool, measured, result)?, admitted)
            };
            match completion {
                QueryCompletion::Complete => break,
                QueryCompletion::CapabilityChanged => {
                    self.invalidate_store_plans(&database);
                }
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
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

        let mut attempted = unavailable_databases.clone();
        while let Some((database, generation, schema)) = selected_store_plans_info(
            &self.capabilities,
            self.server_database.as_deref(),
            &attempted,
        ) {
            let is_current = self.server_database.as_deref() == Some(&database);
            let (completion, admitted) = 'query: {
                let Some(pool) = self.database_pool_mut(&database) else {
                    break 'query (QueryCompletion::ConnectionFailed, false);
                };
                let connection = pool.connection_label(0);
                let session = match session_for_generation(pool, generation, observe) {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        break 'query (QueryCompletion::ConnectionFailed, false);
                    }
                    Err(QueryFailure::ServerTimeout) => {
                        break 'query (QueryCompletion::ServerTimedOut, false);
                    }
                    Err(QueryFailure::Source) => {
                        break 'query (QueryCompletion::SourceFailed, false);
                    }
                };
                let mut measured = measure(observe, "pg_store_plans_info", &connection, &database);
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    store_plans_info::collect(
                        session,
                        &schema.qualify("pg_store_plans_info"),
                        measured.stats_mut(),
                    ),
                )
                .await;
                match result {
                    Ok(Ok(row)) => {
                        measured = deliver(
                            measured,
                            admit,
                            PgBatch::StorePlansInfo(row),
                            settings.cloned(),
                        )?;
                        measured.success();
                        (QueryCompletion::Complete, true)
                    }
                    other => (finish_failed(measured, other), false),
                }
            };
            match completion {
                QueryCompletion::Complete => break,
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
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
                QueryCompletion::CapabilityChanged => {
                    self.invalidate_store_plans_info(&database);
                }
                QueryCompletion::ServerTimedOut | QueryCompletion::SourceFailed => {}
            }
            if !try_another_database(completion, admitted) {
                break;
            }
            attempted.insert(database);
        }
        Ok(true)
    }

    async fn refresh_secondary_capabilities(
        &mut self,
        server_major: u32,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> BTreeSet<String> {
        let mut unavailable = BTreeSet::new();
        let found = self.discovered.clone();
        for database in found {
            if self.server_database.as_deref() == Some(database.name.as_str()) {
                continue;
            }
            let cached_generation = self
                .capabilities
                .get(&database.name)
                .map(|capabilities| capabilities.generation);
            let live_generation = self
                .databases
                .get(&database.name)
                .and_then(Pool::generation);
            if capabilities_match_generation(cached_generation, live_generation) {
                continue;
            }
            let refreshed = {
                let Some(pool) = self.databases.get_mut(&database.name) else {
                    continue;
                };
                let connection = pool.connection_label(0);
                let session = match open_session(pool, observe).await {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        unavailable.insert(database.name.clone());
                        self.capabilities.remove(&database.name);
                        continue;
                    }
                    Err(QueryFailure::ServerTimeout) => {
                        unavailable.insert(database.name.clone());
                        continue;
                    }
                    Err(QueryFailure::Source) => {
                        self.capabilities.remove(&database.name);
                        continue;
                    }
                };
                let generation = session.generation();
                let mut measured =
                    measure(observe, "extension_inventory", &connection, &database.name);
                let result = query::timeout(
                    session,
                    QUERY_TIMEOUT,
                    extension::inventory(session, measured.stats_mut()),
                )
                .await;
                (generation, finish_query(measured, result))
            };
            match refreshed {
                (generation, Ok(inventory)) => {
                    warn_outdated_statements_layout(&inventory, server_major, &database.name);
                    self.capabilities.insert(
                        database.name,
                        capabilities_from_inventory(&inventory, generation, server_major),
                    );
                }
                (_generation, Err(QueryFailure::Timeout | QueryFailure::Connection)) => {
                    if let Some(pool) = self.databases.get_mut(&database.name) {
                        pool.close();
                    }
                    unavailable.insert(database.name.clone());
                    self.capabilities.remove(&database.name);
                }
                (_generation, Err(QueryFailure::ServerTimeout)) => {
                    // The live session is still usable, but this database's
                    // stale generation must not be selected in this cycle.
                    unavailable.insert(database.name.clone());
                }
                (generation, Err(QueryFailure::Source)) => {
                    self.capabilities.insert(
                        database.name,
                        DatabaseCapabilities {
                            generation,
                            ..DatabaseCapabilities::default()
                        },
                    );
                }
            }
        }
        unavailable
    }

    async fn collect_relations<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        let found = self.discovered.clone();
        for database in &found {
            let is_current = self.server_database.as_deref() == Some(database.name.as_str());
            let result = {
                let Some(pool) = self.database_pool_mut(&database.name) else {
                    continue;
                };
                collect_relation_database(
                    pool,
                    database,
                    probe.major,
                    is_current.then_some(probe.generation),
                    observe,
                    cached_settings,
                    admit,
                )
                .await
            };
            match result {
                Err(error) => {
                    return Err(error);
                }
                Ok(RelationResult::Complete | RelationResult::SourceFailed) => {}
                Ok(RelationResult::ConnectionFailed | RelationResult::TimedOut) if is_current => {
                    self.clear_primary_connection();
                    return Ok(());
                }
                Ok(RelationResult::ConnectionFailed | RelationResult::TimedOut) => {
                    if let Some(pool) = self.database_pool_mut(&database.name) {
                        pool.close();
                    }
                }
            }
        }
        Ok(())
    }

    /// Refresh the connectable database list and one capability map entry per
    /// database. A failed inventory never leaves a stale capability behind.
    #[allow(
        clippy::too_many_lines,
        reason = "database enumeration and per-database inventory form one atomic refresh"
    )]
    async fn discover(
        &mut self,
        probe: &GenerationProbe,
        now: Instant,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> bool {
        self.last_discovery = Some(now);
        let found = {
            let Some(server) = self.server.as_mut() else {
                return false;
            };
            let connection = server.connection_label(0);
            let session = match session_for_generation(server, probe.generation, observe) {
                Ok(session) => session,
                Err(_failure) => {
                    self.clear_primary_connection();
                    return false;
                }
            };
            let mut measured = measure(observe, "databases", &connection, &probe.database);
            let result = query::timeout(
                session,
                QUERY_TIMEOUT,
                databases::enumerate(session, measured.stats_mut()),
            )
            .await;
            match finish_query(measured, result) {
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
                    self.last_discovery = None;
                    return false;
                }
            }
        };
        self.server_database = found
            .iter()
            .find(|database| database.is_current)
            .map(|database| database.name.clone());
        if let Some(server) = self.server.as_ref() {
            databases::refresh(&mut self.databases, &found, server);
        }

        let previous_capabilities = self.capabilities.clone();
        let mut capabilities = BTreeMap::new();
        for database in &found {
            let Some(pool) = self.database_pool_mut(&database.name) else {
                continue;
            };
            let connection = pool.connection_label(0);
            let session = if database.is_current {
                session_for_generation(pool, probe.generation, observe)
            } else {
                open_session(pool, observe).await
            };
            let session = match session {
                Ok(session) => session,
                Err(failure) => {
                    match failure {
                        QueryFailure::Timeout | QueryFailure::Connection => {
                            pool.close();
                            if database.is_current {
                                self.clear_primary_connection();
                                return false;
                            }
                        }
                        QueryFailure::ServerTimeout => {
                            let generation = pool.generation();
                            let cached = previous_capabilities
                                .get(&database.name)
                                .filter(|cached| Some(cached.generation) == generation)
                                .cloned();
                            self.last_discovery = None;
                            if let Some(cached) = cached {
                                capabilities.insert(database.name.clone(), cached);
                            }
                        }
                        QueryFailure::Source => {}
                    }
                    continue;
                }
            };
            let generation = session.generation();
            let mut measured = measure(observe, "extension_inventory", &connection, &database.name);
            let result = query::timeout(
                session,
                QUERY_TIMEOUT,
                extension::inventory(session, measured.stats_mut()),
            )
            .await;
            match finish_query(measured, result) {
                Ok(inventory) => {
                    warn_outdated_statements_layout(&inventory, probe.major, &database.name);
                    capabilities.insert(
                        database.name.clone(),
                        capabilities_from_inventory(&inventory, generation, probe.major),
                    );
                }
                Err(failure) => match failure {
                    QueryFailure::Timeout | QueryFailure::Connection => {
                        pool.close();
                        if database.is_current {
                            self.clear_primary_connection();
                            return false;
                        }
                    }
                    QueryFailure::ServerTimeout => {
                        self.last_discovery = None;
                        if let Some(cached) = previous_capabilities
                            .get(&database.name)
                            .filter(|cached| cached.generation == generation)
                        {
                            capabilities.insert(database.name.clone(), cached.clone());
                        }
                    }
                    QueryFailure::Source => {
                        capabilities.insert(
                            database.name.clone(),
                            DatabaseCapabilities {
                                generation,
                                ..DatabaseCapabilities::default()
                            },
                        );
                    }
                },
            }
        }
        self.discovered = found;
        self.capabilities = capabilities;
        true
    }

    fn database_pool_mut(&mut self, database: &str) -> Option<&mut Pool> {
        if self.server_database.as_deref() == Some(database) {
            self.server.as_mut()
        } else {
            self.databases.get_mut(database)
        }
    }

    fn invalidate_statements(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.statements = None;
        }
        self.last_discovery = None;
    }

    fn invalidate_statements_info(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.statements_info = None;
        }
        self.last_discovery = None;
    }

    fn invalidate_store_plans(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.store_plans = None;
        }
        self.last_discovery = None;
    }

    fn invalidate_store_plans_info(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.store_plans_info = None;
        }
        self.last_discovery = None;
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

async fn read_generation_probe(
    session: Session<'_>,
    generation: u64,
    stats: &mut query::QueryStats,
) -> anyhow::Result<GenerationProbe> {
    let mut rows = query::read_simple_rows(session, SERVER_PROBE_SQL, stats, |row| {
        let version = row
            .get(0)
            .context("server probe omitted server_version_num")?;
        let datid = row
            .get(1)
            .context("server probe omitted current database oid")?;
        let database = row
            .get(2)
            .context("server probe omitted current_database")?;
        let usesysid = row
            .get(3)
            .context("server probe omitted session role oid")?;
        let user = row.get(4).context("server probe omitted session_user")?;
        let visibility = row
            .get(5)
            .context("server probe omitted pg_read_all_stats usability")?;
        let version = version
            .parse::<u32>()
            .context("parse server_version_num from server probe")?;
        let datid = datid
            .parse::<u32>()
            .context("parse current database oid from server probe")?;
        let usesysid = usesysid
            .parse::<u32>()
            .context("parse session role oid from server probe")?;
        let full_visibility = match visibility {
            "t" => true,
            "f" => false,
            other => anyhow::bail!("server probe returned {other:?} for visibility"),
        };
        Ok(GenerationProbe {
            generation,
            major: version / 10_000,
            datid,
            database: database.to_owned(),
            usesysid,
            user: user.to_owned(),
            full_visibility,
        })
    })
    .await?;
    anyhow::ensure!(rows.len() == 1, "server probe returned {} rows", rows.len());
    Ok(rows.remove(0))
}

fn discovery_due(last: Option<Instant>, now: Instant, forced: bool) -> bool {
    forced || last.is_none_or(|last| now.saturating_duration_since(last) >= DISCOVERY_INTERVAL)
}

fn capabilities_from_inventory(
    inventory: &[extension::InventoryEntry],
    generation: u64,
    server_major: u32,
) -> DatabaseCapabilities {
    let statements_entry = inventory
        .iter()
        .find(|entry| entry.name == statements::EXTENSION);
    let store_plans_entry = inventory
        .iter()
        .find(|entry| entry.name == store_plans::EXTENSION);
    DatabaseCapabilities {
        generation,
        statements: statements_entry.and_then(|entry| statements::capability(entry, server_major)),
        store_plans: store_plans_entry.and_then(store_plans::capability),
        statements_info: statements_entry
            .filter(|entry| entry.schema_usable && entry.statements_info)
            .map(|entry| entry.schema.clone()),
        store_plans_info: store_plans_entry
            .filter(|entry| entry.schema_usable && entry.store_plans_info)
            .map(|entry| entry.schema.clone()),
    }
}

fn warn_outdated_statements_layout(
    inventory: &[extension::InventoryEntry],
    server_major: u32,
    database: &str,
) {
    if server_major < 14 {
        return;
    }
    let Some(entry) = inventory
        .iter()
        .find(|entry| entry.name == statements::EXTENSION)
    else {
        return;
    };
    let outdated = extension::parse_version(&entry.extversion)
        .and_then(statements::statements_version)
        .is_some_and(|version| matches!(version, StatementsVersion::V1 | StatementsVersion::V2));
    if outdated {
        log_event(
            LogLevel::Warn,
            "pg_stat_statements_extension_update_required",
            &[
                field("database", database),
                field("extension_version", &entry.extversion),
                field("reason", "run ALTER EXTENSION pg_stat_statements UPDATE"),
            ],
        );
    }
}

fn selected_capability<T: Clone>(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
    get: impl Fn(&DatabaseCapabilities) -> Option<&T>,
) -> Option<(String, u64, T)> {
    if let Some(current) = current_database
        && !excluded.contains(current)
        && let Some(capabilities) = capabilities.get(current)
        && let Some(capability) = get(capabilities)
    {
        return Some((
            current.to_owned(),
            capabilities.generation,
            capability.clone(),
        ));
    }
    capabilities
        .iter()
        .filter(|(database, _capabilities)| !excluded.contains(*database))
        .find_map(|(database, capabilities)| {
            get(capabilities).map(|capability| {
                (
                    database.clone(),
                    capabilities.generation,
                    capability.clone(),
                )
            })
        })
}

const fn capabilities_match_generation(cached: Option<u64>, live: Option<u64>) -> bool {
    matches!((cached, live), (Some(cached), Some(live)) if cached == live)
}

fn selected_statements(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, statements::StatementsCapability)> {
    let richest = capabilities
        .iter()
        .filter(|(database, _capabilities)| !excluded.contains(*database))
        .filter_map(|(_database, entry)| entry.statements.as_ref())
        .map(|capability| statements_rank(capability.version))
        .max()?;
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry
            .statements
            .as_ref()
            .filter(|capability| statements_rank(capability.version) == richest)
    })
}

const fn statements_rank(version: StatementsVersion) -> u8 {
    match version {
        StatementsVersion::V1 => 1,
        StatementsVersion::V2 => 2,
        StatementsVersion::V3 => 3,
        StatementsVersion::V4 => 4,
        StatementsVersion::V5 => 5,
        StatementsVersion::V6 => 6,
    }
}

fn selected_statements_info(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, extension::ExtensionSchema)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.statements_info.as_ref()
    })
}

fn selected_store_plans(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, store_plans::StorePlansCapability)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.store_plans.as_ref()
    })
}

fn selected_store_plans_info(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    excluded: &BTreeSet<String>,
) -> Option<(String, u64, extension::ExtensionSchema)> {
    selected_capability(capabilities, current_database, excluded, |entry| {
        entry.store_plans_info.as_ref()
    })
}

const fn try_another_database(completion: QueryCompletion, admitted: bool) -> bool {
    !admitted
        && !matches!(
            completion,
            QueryCompletion::Complete | QueryCompletion::ServerTimedOut
        )
}

fn settings_equal_ignoring_ts(left: &[SettingsRow], right: &[SettingsRow]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.datid == right.datid
                && left.datname == right.datname
                && left.usesysid == right.usesysid
                && left.usename == right.usename
                && left.name == right.name
                && left.setting == right.setting
                && left.unit == right.unit
                && left.source == right.source
                && left.sourcefile == right.sourcefile
                && left.sourceline == right.sourceline
                && left.pending_restart == right.pending_restart
                && left.context == right.context
                && left.vartype == right.vartype
                && left.boot_val == right.boot_val
                && left.reset_val == right.reset_val
        })
}

fn cached_settings_for_generation(
    settings: Option<&CachedSettings>,
    generation: u64,
) -> Option<Arc<[SettingsRow]>> {
    settings
        .filter(|cached| cached.generation == generation)
        .map(|cached| Arc::clone(&cached.rows))
}

async fn read_settings<E>(
    server: &mut Pool,
    probe: &GenerationProbe,
    cached: Option<&[SettingsRow]>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<Option<Arc<[SettingsRow]>>, E> {
    let database = server.database_label().to_owned();
    let connection = server.connection_label(0);
    let session = match session_for_generation(server, probe.generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(None),
    };
    let mut measured = measure(observe, "pg_settings", &connection, &database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        settings::collect(
            session,
            measured.stats_mut(),
            probe.datid,
            &probe.database,
            probe.usesysid,
            &probe.user,
        ),
    )
    .await;
    match result {
        Ok(Ok(rows)) => {
            let rows: Arc<[SettingsRow]> = Arc::from(rows);
            if cached.is_none_or(|cached| !settings_equal_ignoring_ts(cached, &rows)) {
                measured = deliver(measured, admit, PgBatch::Settings(Arc::clone(&rows)), None)?;
            }
            measured.success();
            Ok(Some(rows))
        }
        other => {
            if matches!(
                finish_failed(measured, other),
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut
            ) {
                server.close();
            }
            Ok(None)
        }
    }
}

async fn collect_bgwriter<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_stat_bgwriter", &connection, database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        bgwriter::collect_bgwriter(session, major, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(row)) => {
            measured = deliver(measured, admit, PgBatch::Bgwriter(row), settings.cloned())?;
            measured.success();
            Ok(true)
        }
        other => Ok(fixed_source_can_continue(finish_failed(measured, other))),
    }
}

async fn collect_checkpointer<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let Some(_version) = checkpointer::checkpointer_version(major) else {
        return Ok(true);
    };
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_stat_checkpointer", &connection, database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        checkpointer::collect_checkpointer(session, major, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(Some(row))) => {
            measured = deliver(
                measured,
                admit,
                PgBatch::Checkpointer(row),
                settings.cloned(),
            )?;
            measured.success();
            Ok(true)
        }
        Ok(Ok(None)) => {
            measured.success();
            Ok(true)
        }
        other => Ok(fixed_source_can_continue(finish_failed(measured, other))),
    }
}

async fn collect_wal_storage<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    if wal_storage::wal_storage_query(major).is_none() {
        return Ok(true);
    }
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_wal_storage", &connection, database);
    let result = query::timeout(
        session,
        QUERY_TIMEOUT,
        wal_storage::collect(session, major, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(Some(row))) => {
            measured = deliver(measured, admit, PgBatch::WalStorage(row), settings.cloned())?;
            measured.success();
            Ok(true)
        }
        Ok(Ok(None)) => {
            measured.success();
            Ok(true)
        }
        other => Ok(fixed_source_can_continue(finish_failed(measured, other))),
    }
}

async fn collect_locks<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let version = locks::locks_version(major);
    let mut measured = measure(observe, "pg_locks", &connection, database);
    let result = locks::collect_locks(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Locks(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_prepared<E>(
    pool: &mut Pool,
    generation: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_prepared_xacts", &connection, database);
    let result = prepared_xacts::collect_prepared_xacts(session, measured.stats_mut(), |batch| {
        admit(PgBatch::PreparedXacts(batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_database<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database_label: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let version = database::database_version(major);
    let mut measured = measure(observe, "pg_stat_database", &connection, database_label);
    let result = database::collect_database(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Database(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_io<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let Some(version) = io::io_version(major) else {
        return Ok(true);
    };
    let mut measured = measure(observe, "pg_stat_io", &connection, database);
    let result = io::collect_io(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Io(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_activity<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let version = activity::activity_version(major);
    let mut measured = measure(observe, "pg_stat_activity", &connection, database);
    let result = activity::collect_activity(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Activity(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_progress<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let connection = pool.connection_label(0);
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_stat_progress_vacuum", &connection, database);
    let result =
        progress_vacuum::collect_progress_vacuum(session, major, measured.stats_mut(), |batch| {
            admit(PgBatch::ProgressVacuum(batch.rows), settings.cloned())
        })
        .await;
    finish_batched(pool, measured, result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelationResult {
    Complete,
    SourceFailed,
    ConnectionFailed,
    TimedOut,
}

async fn collect_relation_database<E>(
    pool: &mut Pool,
    database: &databases::Database,
    major: u32,
    expected_generation: Option<u64>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<RelationResult, E> {
    let connection = pool.connection_label(0);
    let session = match match expected_generation {
        Some(generation) => session_for_generation(pool, generation, observe),
        None => open_session(pool, observe).await,
    } {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Source | QueryFailure::ServerTimeout) => {
            return Ok(RelationResult::SourceFailed);
        }
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
    };
    let generation = session.generation();
    let mut source_failed = false;
    let table_version = user_tables::user_tables_version(major);
    let mut measured = measure(observe, "pg_stat_user_tables", &connection, &database.name);
    let result =
        user_tables::collect_user_tables(session, database, major, measured.stats_mut(), |batch| {
            admit(
                PgBatch::UserTables(table_version, batch.rows),
                settings.cloned(),
            )
        })
        .await;
    match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete => {}
        QueryCompletion::SourceFailed
        | QueryCompletion::CapabilityChanged
        | QueryCompletion::ServerTimedOut => source_failed = true,
        QueryCompletion::ConnectionFailed => return Ok(RelationResult::ConnectionFailed),
        QueryCompletion::TimedOut => return Ok(RelationResult::TimedOut),
    }

    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Source | QueryFailure::ServerTimeout) => {
            return Ok(RelationResult::SourceFailed);
        }
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
    };
    let index_version = user_indexes::user_indexes_version(major);
    let mut measured = measure(observe, "pg_stat_user_indexes", &connection, &database.name);
    let result = user_indexes::collect_user_indexes(
        session,
        database,
        major,
        measured.stats_mut(),
        |batch| {
            admit(
                PgBatch::UserIndexes(index_version, batch.rows),
                settings.cloned(),
            )
        },
    )
    .await;
    Ok(match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete if source_failed => RelationResult::SourceFailed,
        QueryCompletion::Complete => RelationResult::Complete,
        QueryCompletion::SourceFailed
        | QueryCompletion::CapabilityChanged
        | QueryCompletion::ServerTimedOut => RelationResult::SourceFailed,
        QueryCompletion::ConnectionFailed => RelationResult::ConnectionFailed,
        QueryCompletion::TimedOut => RelationResult::TimedOut,
    })
}

fn deliver<'a, E>(
    mut measured: QueryMeasurement<'a>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    batch: PgBatch,
    settings: Option<Arc<[SettingsRow]>>,
) -> Result<QueryMeasurement<'a>, E> {
    let started = Instant::now();
    match admit(batch, settings) {
        Ok(write) => {
            measured
                .stats_mut()
                .record_batch_write(started.elapsed(), write);
            Ok(measured)
        }
        Err(error) => {
            measured.stats_mut().record_failed_batch(started.elapsed());
            measured.sink_error();
            Err(error)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryFailure {
    Source,
    Connection,
    ServerTimeout,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryCompletion {
    Complete,
    SourceFailed,
    CapabilityChanged,
    ConnectionFailed,
    ServerTimedOut,
    TimedOut,
}

const fn fixed_source_can_continue(completion: QueryCompletion) -> bool {
    matches!(
        completion,
        QueryCompletion::SourceFailed
            | QueryCompletion::CapabilityChanged
            | QueryCompletion::ServerTimedOut
    )
}

async fn open_session<'a>(
    pool: &'a mut Pool,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<Session<'a>, QueryFailure> {
    let database = pool.database_label().to_owned();
    let connection = pool.connection_label(0);
    let started = Instant::now();
    match pool.session().await {
        Ok(session) => Ok(session),
        Err(error) => {
            let timeout = error.is_timeout();
            observe(PgObservation::Connection(ConnectionObservation {
                connection,
                database,
                elapsed: started.elapsed(),
                timeout,
                closed: false,
                error: error.to_string(),
            }));
            if timeout {
                Err(QueryFailure::Timeout)
            } else {
                Err(QueryFailure::Connection)
            }
        }
    }
}

fn session_for_generation<'a>(
    pool: &'a mut Pool,
    expected: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<Session<'a>, QueryFailure> {
    let database = pool.database_label().to_owned();
    let connection = pool.connection_label(0);
    let Some(session) = pool.session_for_generation(expected) else {
        observe(PgObservation::Connection(ConnectionObservation {
            connection,
            database,
            elapsed: Duration::ZERO,
            timeout: false,
            closed: true,
            error: "connection closed before the query started".to_owned(),
        }));
        return Err(QueryFailure::Connection);
    };
    Ok(session)
}

fn finish_query<T>(
    measured: QueryMeasurement<'_>,
    result: Result<anyhow::Result<T>, tokio::time::error::Elapsed>,
) -> Result<T, QueryFailure> {
    match result {
        Ok(Ok(value)) => {
            measured.success();
            Ok(value)
        }
        Ok(Err(error)) => {
            if postgres_query_cancelled(&error) {
                measured.server_timeout(format!("{error:#}"));
                Err(QueryFailure::ServerTimeout)
            } else if postgres_connection_error(&error) {
                measured.error(format!("{error:#}"));
                Err(QueryFailure::Connection)
            } else {
                measured.error(format!("{error:#}"));
                Err(QueryFailure::Source)
            }
        }
        Err(_elapsed) => {
            measured.timeout();
            Err(QueryFailure::Timeout)
        }
    }
}

fn finish_failed<T>(
    measured: QueryMeasurement<'_>,
    result: Result<anyhow::Result<T>, tokio::time::error::Elapsed>,
) -> QueryCompletion {
    match result {
        Ok(Ok(_value)) => {
            measured.success();
            QueryCompletion::Complete
        }
        Ok(Err(error)) => {
            if postgres_query_cancelled(&error) {
                measured.server_timeout(format!("{error:#}"));
                QueryCompletion::ServerTimedOut
            } else if postgres_connection_error(&error) {
                measured.error(format!("{error:#}"));
                QueryCompletion::ConnectionFailed
            } else if postgres_capability_changed(&error) {
                measured.error(format!("{error:#}"));
                QueryCompletion::CapabilityChanged
            } else {
                measured.error(format!("{error:#}"));
                QueryCompletion::SourceFailed
            }
        }
        Err(_elapsed) => {
            measured.timeout();
            QueryCompletion::TimedOut
        }
    }
}

fn finish_batched<E>(
    pool: &mut Pool,
    measured: QueryMeasurement<'_>,
    result: Result<(), BatchError<E>>,
) -> Result<bool, E> {
    Ok(matches!(
        finish_batched_kind(pool, measured, result)?,
        QueryCompletion::Complete
            | QueryCompletion::SourceFailed
            | QueryCompletion::CapabilityChanged
            | QueryCompletion::ServerTimedOut
    ))
}

/// Formats the error chain because `tokio_postgres::Error` displays only its
/// top-level message.
fn error_text(error: &(dyn std::error::Error + '_)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(current) = source {
        let text = current.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = current.source();
    }
    message
}

fn finish_batched_kind<E>(
    pool: &mut Pool,
    measured: QueryMeasurement<'_>,
    result: Result<(), BatchError<E>>,
) -> Result<QueryCompletion, E> {
    match result {
        Ok(()) => {
            measured.success();
            Ok(QueryCompletion::Complete)
        }
        Err(BatchError::PostgreSql(error)) => {
            if query::is_query_cancelled(&error) {
                measured.server_timeout(error_text(&error));
                Ok(QueryCompletion::ServerTimedOut)
            } else if postgres_stream_connection_error(&error) {
                measured.error(error_text(&error));
                pool.close();
                Ok(QueryCompletion::ConnectionFailed)
            } else if postgres_stream_capability_changed(&error) {
                measured.error(error_text(&error));
                Ok(QueryCompletion::CapabilityChanged)
            } else {
                measured.error(error_text(&error));
                Ok(QueryCompletion::SourceFailed)
            }
        }
        Err(BatchError::Decode(error)) => {
            measured.error(format!("{error:#}"));
            pool.close();
            Ok(QueryCompletion::SourceFailed)
        }
        Err(BatchError::Sink(error)) => {
            measured.sink_error();
            // Dropping an unconsumed RowStream leaves a response in flight.
            // Closing prevents a later query from being written behind it.
            pool.close();
            Err(error)
        }
        Err(BatchError::Timeout) => {
            measured.timeout();
            pool.close();
            Ok(QueryCompletion::TimedOut)
        }
    }
}

fn postgres_stream_connection_error(error: &tokio_postgres::Error) -> bool {
    error.is_closed() || error.as_db_error().is_none()
}

fn postgres_stream_capability_changed(error: &tokio_postgres::Error) -> bool {
    error
        .code()
        .is_some_and(|code| capability_sqlstate(code.code()))
}

fn capability_sqlstate(code: &str) -> bool {
    matches!(
        code,
        "42P01" | "42883" | "42704" | "42703" | "3F000" | "42501"
    )
}

fn postgres_connection_error(error: &anyhow::Error) -> bool {
    postgres_error_matches(error, postgres_stream_connection_error)
}

fn postgres_capability_changed(error: &anyhow::Error) -> bool {
    postgres_error_matches(error, postgres_stream_capability_changed)
}

pub(crate) fn postgres_query_cancelled(error: &anyhow::Error) -> bool {
    postgres_error_matches(error, query::is_query_cancelled)
}

fn postgres_error_matches(
    error: &anyhow::Error,
    predicate: fn(&tokio_postgres::Error) -> bool,
) -> bool {
    if error
        .chain()
        .any(<dyn std::error::Error>::is::<query::DecodeError>)
    {
        return false;
    }
    if let Some(stream) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<query::StreamError>())
    {
        return predicate(stream.postgres());
    }
    error.chain().any(|cause| {
        cause
            .downcast_ref::<tokio_postgres::Error>()
            .is_some_and(predicate)
    })
}

#[cfg(test)]
mod tests;
