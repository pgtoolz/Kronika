//! Asking a server which log it writes and who it is.
//!
//! Both `PostgreSQL` and `PgBouncer` know where their own log goes, so the
//! collector asks them rather than having the operator declare it a second
//! time. What comes back also carries the identity that ends up in every row.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr as _;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures_util::TryStreamExt as _;
use kronika_source_pg::Session;
use kronika_source_pg::query::{self, QueryStats};
use tokio_postgres::config::Host;
use tokio_postgres::{Config, NoTls, SimpleQueryMessage};

use crate::pg_sources::{
    ConnectionObservation, PgObservation, QueryObservation, QueryOutcome, postgres_query_cancelled,
};

const DEFAULT_PORT: u16 = 5432;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = query::QUERY_FETCH_TIMEOUT;

const POSTGRES_LOG_FACTS_QUERY: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " bins/kronika-collector/src/log_sources/settings.rs */ ",
    "SELECT session_user AS user_name, current_database() AS database_name, ",
    "current_setting('log_line_prefix') AS line_prefix, ",
    "current_setting('data_directory') AS data_directory, ",
    "pg_current_logfile() AS log_path"
);
const POSTGRES_SYSTEM_IDENTIFIER_QUERY: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " bins/kronika-collector/src/log_sources/settings.rs */ ",
    "SELECT system_identifier::text AS system_identifier FROM pg_control_system()"
);

/// A parsed connection and the only identity safe to put in a log line.
pub(super) struct ConnectionTarget {
    config: Config,
    label: String,
    source_index: usize,
}

impl ConnectionTarget {
    /// Parse one configured connection without retaining its original text.
    pub(super) fn parse(raw: &str, source_index: usize) -> Result<Self, InvalidConnection> {
        let config = Config::from_str(raw).map_err(|_error| InvalidConnection)?;
        validate_endpoints(&config)?;
        let label = connection_label(&config);
        Ok(Self {
            config,
            label,
            source_index,
        })
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn database_label(&self) -> &str {
        self.config.get_dbname().unwrap_or("server-default")
    }

    fn label_for_user(&self, user: &str) -> String {
        connection_label_for_user(&self.config, user)
    }

    pub(super) const fn source_index(&self) -> usize {
        self.source_index
    }
}

impl fmt::Debug for ConnectionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionTarget")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Deliberately carries neither parser details nor the rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InvalidConnection;

fn validate_endpoints(config: &Config) -> Result<(), InvalidConnection> {
    let hosts = config.get_hosts().len();
    let hostaddrs = config.get_hostaddrs().len();
    let endpoints = hosts.max(hostaddrs);
    let ports = config.get_ports().len();
    if endpoints == 0
        || (hosts != 0 && hostaddrs != 0 && hosts != hostaddrs)
        || !matches!(ports, 0 | 1) && ports != endpoints
    {
        return Err(InvalidConnection);
    }
    Ok(())
}

fn connection_label(config: &Config) -> String {
    connection_label_for_user(config, config.get_user().unwrap_or("server-default"))
}

fn connection_label_for_user(config: &Config, user: &str) -> String {
    let ports = config.get_ports();
    if config.get_hosts().is_empty() {
        return config
            .get_hostaddrs()
            .iter()
            .enumerate()
            .map(|(index, host)| endpoint(user, &ip_label(*host), port_at(ports, index)))
            .collect::<Vec<_>>()
            .join(",");
    }
    config
        .get_hosts()
        .iter()
        .enumerate()
        .map(|(index, host)| {
            let host = match host {
                Host::Tcp(host) => tcp_label(host),
                #[cfg(unix)]
                Host::Unix(path) => format!("unix:{}", path.display()),
            };
            endpoint(user, &host, port_at(ports, index))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn port_at(ports: &[u16], index: usize) -> u16 {
    match ports {
        [] => DEFAULT_PORT,
        [port] => *port,
        many => many.get(index).copied().unwrap_or(DEFAULT_PORT),
    }
}

fn endpoint(user: &str, host: &str, port: u16) -> String {
    format!("{user}@{host}:{port}")
}

fn tcp_label(host: &str) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        return host.to_owned();
    }
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn ip_label(host: IpAddr) -> String {
    match host {
        IpAddr::V4(host) => host.to_string(),
        IpAddr::V6(host) => format!("[{host}]"),
    }
}

/// What one `PostgreSQL` server said about itself.
#[derive(Debug)]
pub(super) struct PostgresServer {
    /// The file the server is writing right now, or `None` when
    /// `logging_collector` is off and there is no file to follow.
    pub(super) log_path: Option<String>,
    /// The layout of a `stderr` line's prefix.
    pub(super) line_prefix: String,
    /// Generated at `initdb`, so it survives restarts, renames and moves.
    pub(super) system_identifier: Option<u64>,
}

/// What one `PgBouncer` said about itself.
#[derive(Debug)]
pub(super) struct PgBouncerServer {
    /// Empty when `logfile` is unset, which means the pooler writes to stderr
    /// and there is no file to follow.
    pub(super) log_path: Option<String>,
}

#[derive(Debug)]
struct LogFacts {
    user_name: String,
    database_name: String,
    line_prefix: String,
    data_directory: String,
    log_path: Option<String>,
}

/// Ask `PostgreSQL` for its log file, its line layout and its identity.
///
/// # Errors
///
/// Returns an error when the connection or the refresh query fails. A failed
/// first identity query leaves the identity empty so the next rescan retries it.
#[allow(
    clippy::too_many_lines,
    reason = "connection, fact-query, and optional identity-query outcomes are reported together"
)]
pub(super) async fn postgres(
    target: &ConnectionTarget,
    cached_system_identifier: Option<u64>,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<PostgresServer> {
    let connect_started = Instant::now();
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, target.config.connect(NoTls)).await;
    let (client, connection) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(error)) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: false,
                closed: false,
                error: error.to_string(),
            }));
            return Err(error).context("connect to PostgreSQL");
        }
        Err(elapsed) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: true,
                closed: false,
                error: format!(
                    "connect to PostgreSQL timed out after {} seconds",
                    CONNECT_TIMEOUT.as_secs()
                ),
            }));
            return Err(elapsed).context("connect to PostgreSQL timed out");
        }
    };
    // The connection drives the protocol and ends when the client is dropped.
    let driver = tokio::spawn(connection);
    match tokio::time::timeout(CONNECT_TIMEOUT, query::configure_session(&client)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: false,
                closed: false,
                error: error.to_string(),
            }));
            drop(client);
            driver.abort();
            return Err(error).context("configure PostgreSQL monitoring session");
        }
        Err(elapsed) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: true,
                closed: false,
                error: format!(
                    "configure PostgreSQL monitoring session timed out after {} seconds",
                    CONNECT_TIMEOUT.as_secs()
                ),
            }));
            drop(client);
            driver.abort();
            return Err(elapsed).context("configure PostgreSQL monitoring session timed out");
        }
    }
    let mut facts_stats = QueryStats::default();
    let facts_started = Instant::now();
    let session = Session::new(&client, 0);
    let facts = query::timeout(
        session,
        QUERY_TIMEOUT,
        read_log_facts(session, &mut facts_stats),
    )
    .await;
    let facts = match facts {
        Ok(Ok(facts)) => facts,
        Ok(Err(error)) => {
            let outcome = if postgres_query_cancelled(&error) {
                QueryOutcome::Timeout
            } else {
                QueryOutcome::Error
            };
            observe_query(
                observe,
                "postgres_log_facts",
                target.label(),
                target.database_label(),
                facts_started,
                facts_stats,
                outcome,
                Some(format!("{error:#}")),
            );
            drop(client);
            driver.abort();
            return Err(error);
        }
        Err(elapsed) => {
            observe_query(
                observe,
                "postgres_log_facts",
                target.label(),
                target.database_label(),
                facts_started,
                facts_stats,
                QueryOutcome::Timeout,
                Some(format!(
                    "query timed out after {} seconds",
                    QUERY_TIMEOUT.as_secs()
                )),
            );
            drop(client);
            driver.abort();
            return Err(elapsed).context("read PostgreSQL log facts timed out");
        }
    };
    let connection_label = target.label_for_user(&facts.user_name);
    observe_query(
        observe,
        "postgres_log_facts",
        &connection_label,
        &facts.database_name,
        facts_started,
        facts_stats,
        QueryOutcome::Success,
        None,
    );
    let system_identifier = if let Some(identifier) = cached_system_identifier {
        Some(identifier)
    } else {
        let mut stats = QueryStats::default();
        let started = Instant::now();
        let session = Session::new(&client, 0);
        let identity = query::timeout(
            session,
            QUERY_TIMEOUT,
            read_system_identifier(session, &mut stats),
        )
        .await;
        match identity {
            Ok(Ok(identifier)) => {
                observe_query(
                    observe,
                    "postgres_system_identifier",
                    &connection_label,
                    &facts.database_name,
                    started,
                    stats,
                    QueryOutcome::Success,
                    None,
                );
                Some(identifier)
            }
            Ok(Err(error)) => {
                let outcome = if postgres_query_cancelled(&error) {
                    QueryOutcome::Timeout
                } else {
                    QueryOutcome::Error
                };
                observe_query(
                    observe,
                    "postgres_system_identifier",
                    &connection_label,
                    &facts.database_name,
                    started,
                    stats,
                    outcome,
                    Some(format!("{error:#}")),
                );
                None
            }
            Err(_elapsed) => {
                observe_query(
                    observe,
                    "postgres_system_identifier",
                    &connection_label,
                    &facts.database_name,
                    started,
                    stats,
                    QueryOutcome::Timeout,
                    Some(format!(
                        "query timed out after {} seconds",
                        QUERY_TIMEOUT.as_secs()
                    )),
                );
                None
            }
        }
    };
    drop(client);
    driver.abort();
    Ok(PostgresServer {
        log_path: facts
            .log_path
            .map(|name| absolute(&facts.data_directory, &name)),
        line_prefix: facts.line_prefix,
        system_identifier,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the observation keeps query identity, timing, counters, outcome, and error separate"
)]
fn observe_query(
    observe: &mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    connection: &str,
    database: &str,
    started: Instant,
    stats: QueryStats,
    outcome: QueryOutcome,
    error: Option<String>,
) {
    observe(PgObservation::Query(QueryObservation {
        query_name,
        connection: connection.to_owned(),
        database: database.to_owned(),
        elapsed: started.elapsed(),
        stats,
        outcome,
        error,
    }));
}

async fn read_log_facts(session: Session<'_>, stats: &mut QueryStats) -> Result<LogFacts> {
    let stream = session
        .simple_stream(POSTGRES_LOG_FACTS_QUERY, stats)
        .await
        .context("read PostgreSQL log settings")?;
    let mut stream = std::pin::pin!(stream);
    let mut row = None;
    while let Some(message) = stream.try_next().await? {
        stats.received_simple(&message);
        if let SimpleQueryMessage::Row(found) = message {
            row = Some(found);
        }
    }
    let row = row.context("PostgreSQL log settings returned no row")?;
    let user_name = row
        .get("user_name")
        .context("PostgreSQL log settings omitted user_name")?
        .to_owned();
    let database_name = row
        .get("database_name")
        .context("PostgreSQL log settings omitted database_name")?
        .to_owned();
    let line_prefix = row
        .get("line_prefix")
        .context("PostgreSQL log settings omitted line_prefix")?
        .to_owned();
    let data_directory = row
        .get("data_directory")
        .context("PostgreSQL log settings omitted data_directory")?
        .to_owned();
    let log_path = row.get("log_path").map(str::to_owned);
    Ok(LogFacts {
        user_name,
        database_name,
        line_prefix,
        data_directory,
        log_path,
    })
}

async fn read_system_identifier(session: Session<'_>, stats: &mut QueryStats) -> Result<u64> {
    let stream = session
        .simple_stream(POSTGRES_SYSTEM_IDENTIFIER_QUERY, stats)
        .await
        .context("read system_identifier from pg_control_system()")?;
    let mut stream = std::pin::pin!(stream);
    let mut row = None;
    while let Some(message) = stream.try_next().await? {
        stats.received_simple(&message);
        if let SimpleQueryMessage::Row(found) = message {
            row = Some(found);
        }
    }
    let row = row.context("pg_control_system() returned no row")?;
    let identifier = row
        .get("system_identifier")
        .context("pg_control_system() omitted system_identifier")?
        .parse::<i64>()
        .context("parse system_identifier from pg_control_system()")?;
    #[expect(
        clippy::cast_sign_loss,
        reason = "the server reports the identifier as its signed bit pattern"
    )]
    Ok(identifier as u64)
}

/// Ask `PgBouncer` for its log file and where it listens.
///
/// `SHOW CONFIG` needs the account to be in `stats_users` or `admin_users`;
/// no administrative right beyond that.
///
/// # Errors
///
/// Returns an error when the connection cannot be made or the query fails.
pub(super) async fn pgbouncer(
    target: &ConnectionTarget,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<PgBouncerServer> {
    let connect_started = Instant::now();
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, target.config.connect(NoTls)).await;
    let (client, connection) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(error)) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: false,
                closed: false,
                error: error.to_string(),
            }));
            return Err(error).context("connect to PgBouncer");
        }
        Err(elapsed) => {
            observe(PgObservation::Connection(ConnectionObservation {
                connection: target.label().to_owned(),
                database: target.database_label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: true,
                closed: false,
                error: format!(
                    "connect to PgBouncer timed out after {} seconds",
                    CONNECT_TIMEOUT.as_secs()
                ),
            }));
            return Err(elapsed).context("connect to PgBouncer timed out");
        }
    };
    let driver = tokio::spawn(connection);
    let mut stats = QueryStats::default();
    let started = Instant::now();
    let session = Session::new(&client, 0);
    let read = query::timeout(
        session,
        QUERY_TIMEOUT,
        read_pgbouncer_settings(session, &mut stats),
    )
    .await;
    drop(client);
    driver.abort();
    match read {
        Ok(Ok(settings)) => {
            observe_query(
                observe,
                "pgbouncer_show_config",
                target.label(),
                target.database_label(),
                started,
                stats,
                QueryOutcome::Success,
                None,
            );
            Ok(settings)
        }
        Ok(Err(error)) => {
            observe_query(
                observe,
                "pgbouncer_show_config",
                target.label(),
                target.database_label(),
                started,
                stats,
                QueryOutcome::Error,
                Some(format!("{error:#}")),
            );
            Err(error)
        }
        Err(elapsed) => {
            observe_query(
                observe,
                "pgbouncer_show_config",
                target.label(),
                target.database_label(),
                started,
                stats,
                QueryOutcome::Timeout,
                Some(format!(
                    "query timed out after {} seconds",
                    QUERY_TIMEOUT.as_secs()
                )),
            );
            Err(elapsed).context("read SHOW CONFIG from PgBouncer timed out")
        }
    }
}

async fn read_pgbouncer_settings(
    session: Session<'_>,
    stats: &mut QueryStats,
) -> Result<PgBouncerServer> {
    // The admin console speaks the protocol but not the extended query path.
    let stream = session
        .simple_stream("SHOW CONFIG", stats)
        .await
        .context("read SHOW CONFIG from PgBouncer")?;
    let mut stream = std::pin::pin!(stream);
    let mut settings = Settings::default();
    while let Some(message) = stream.try_next().await? {
        stats.received_simple(&message);
        if let SimpleQueryMessage::Row(row) = message {
            settings.take(row.get("key"), row.get("value"));
        }
    }
    Ok(settings.finish())
}

/// The one `SHOW CONFIG` key worth keeping.
#[derive(Debug, Default)]
struct Settings {
    logfile: Option<String>,
}

impl Settings {
    fn take(&mut self, key: Option<&str>, value: Option<&str>) {
        let (Some(key), Some(value)) = (key, value) else {
            return;
        };
        if key == "logfile" {
            self.logfile = Some(value.to_owned());
        }
    }

    fn finish(self) -> PgBouncerServer {
        PgBouncerServer {
            log_path: self.logfile.filter(|path| !path.trim().is_empty()),
        }
    }
}

/// `pg_current_logfile()` reports relative to the data directory.
fn absolute(data_directory: &str, name: &str) -> String {
    if name.starts_with('/') {
        return name.to_owned();
    }
    format!("{}/{name}", data_directory.trim_end_matches('/'))
}

#[cfg(test)]
mod tests;
