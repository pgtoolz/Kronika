//! One reusable connection to a `PostgreSQL` server.
//!
//! A healthy connection remains open across collection cycles for at most one
//! hour. It is also replaced after the driver reports it closed or the caller
//! closes it following a connection, protocol, or deadline failure.

use std::error::Error;
use std::fmt;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use tokio::time::Instant;
use tokio_postgres::config::Host;
use tokio_postgres::{Config, NoTls};

use crate::Session;
use crate::query;

/// Maximum time allowed for opening a `PostgreSQL` connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum lifetime of one healthy frontend session.
pub const MAX_AGE: Duration = Duration::from_hours(1);
const DEFAULT_PORT: u16 = 5432;
static APPLICATION_NAME: OnceLock<String> = OnceLock::new();

/// Failure to open a `PostgreSQL` connection.
#[derive(Debug)]
pub enum ConnectError {
    /// The explicit connection or session-setup deadline elapsed.
    Timeout,
    /// `PostgreSQL` or its transport rejected the connection.
    PostgreSql(tokio_postgres::Error),
}

impl ConnectError {
    /// Whether this failure is the explicit session-opening deadline.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(
                f,
                "open PostgreSQL monitoring session timed out after {} seconds",
                CONNECT_TIMEOUT.as_secs()
            ),
            Self::PostgreSql(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ConnectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Timeout => None,
            Self::PostgreSql(error) => Some(error),
        }
    }
}

/// A connection opened lazily and reused while healthy.
#[derive(Debug)]
pub struct Pool {
    config: Config,
    resolved_user: Option<String>,
    resolved_database: Option<String>,
    open: Option<Open>,
    next_generation: u64,
}

#[derive(Debug)]
struct Open {
    client: tokio_postgres::Client,
    driver: tokio::task::JoinHandle<()>,
    generation: u64,
    opened_at: Instant,
}

impl Open {
    fn reusable(&self) -> bool {
        !self.client.is_closed() && self.opened_at.elapsed() < MAX_AGE
    }
}

impl Pool {
    /// Parse a DSN without opening a connection.
    ///
    /// # Errors
    ///
    /// Returns the parse error when `dsn` is neither a keyword string nor a
    /// connection URL.
    pub fn new(dsn: &str) -> Result<Self> {
        let mut config: Config = dsn.parse().context("parse the PostgreSQL DSN")?;
        config.application_name(collector_application_name());
        Ok(Self::from_config(config))
    }

    /// Build a lazy pool from an already parsed configuration.
    #[must_use]
    pub const fn from_config(config: Config) -> Self {
        Self {
            config,
            resolved_user: None,
            resolved_database: None,
            open: None,
            next_generation: 1,
        }
    }

    /// The same server and credentials, connecting to `dbname` instead.
    #[must_use]
    pub fn on_database(&self, dbname: &str) -> Self {
        let mut config = self.config.clone();
        config.dbname(dbname);
        Self {
            config,
            resolved_user: self.resolved_user.clone(),
            resolved_database: Some(dbname.to_owned()),
            open: None,
            next_generation: 1,
        }
    }

    /// Safe database label used by per-query logs.
    #[must_use]
    pub fn database_label(&self) -> &str {
        self.resolved_database
            .as_deref()
            .or_else(|| self.config.get_dbname())
            .unwrap_or("server-default")
    }

    /// Safe endpoint label with the configured user, or a source placeholder.
    #[must_use]
    pub fn connection_label(&self, source_index: usize) -> String {
        connection_label(
            &self.config,
            self.resolved_user
                .as_deref()
                .or_else(|| self.config.get_user()),
            source_index,
        )
    }

    /// Use the login role and database reported by the server for later logs.
    pub fn remember_resolved_identity(&mut self, user: &str, database: &str) {
        self.resolved_user = Some(user.to_owned());
        self.resolved_database = Some(database.to_owned());
    }

    /// Return the healthy current session, connecting when necessary.
    ///
    /// # Errors
    ///
    /// Returns a finite-deadline timeout or the `PostgreSQL` connection error.
    pub async fn session(&mut self) -> std::result::Result<Session<'_>, ConnectError> {
        if let Some(open) = self.open.take() {
            if open.reusable() {
                let open = self.open.insert(open);
                return Ok(Session::new(&open.client, open.generation));
            }
            open.driver.abort();
        }
        let connected = self.connect().await?;
        let open = self.open.insert(connected);
        Ok(Session::new(&open.client, open.generation))
    }

    /// Return the existing healthy session only when its generation matches.
    ///
    /// This never reconnects. A closed connection is discarded, while a
    /// healthy connection from another generation remains available for the
    /// next collection pass.
    #[must_use]
    pub fn session_for_generation(&mut self, expected: u64) -> Option<Session<'_>> {
        let unusable = self.open.as_ref().is_some_and(|open| !open.reusable());
        if unusable {
            self.close();
            return None;
        }
        let open = self.open.as_ref()?;
        (open.generation == expected).then(|| Session::new(&open.client, open.generation))
    }

    /// Generation of the current healthy connection.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.open
            .as_ref()
            .filter(|open| open.reusable())
            .map(|open| open.generation)
    }

    /// Drop the connection. The next [`session`](Self::session) opens a new one.
    pub fn close(&mut self) {
        if let Some(open) = self.open.take() {
            open.driver.abort();
        }
    }

    async fn connect(&mut self) -> std::result::Result<Open, ConnectError> {
        let connecting = self.config.connect(NoTls);
        let (client, connection) = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_elapsed| ConnectError::Timeout)?
            .map_err(ConnectError::PostgreSql)?;
        let driver = tokio::spawn(async move {
            let _ended = connection.await;
        });
        match tokio::time::timeout(CONNECT_TIMEOUT, query::configure_session(&client)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                driver.abort();
                return Err(ConnectError::PostgreSql(error));
            }
            Err(_elapsed) => {
                driver.abort();
                return Err(ConnectError::Timeout);
            }
        }
        let opened_at = Instant::now();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        Ok(Open {
            client,
            driver,
            generation,
            opened_at,
        })
    }
}

fn collector_application_name() -> &'static str {
    APPLICATION_NAME.get_or_init(|| {
        application_name(
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO),
        )
    })
}

fn application_name(process_id: u32, started: Duration) -> String {
    format!("kronika-collector-{process_id}-{}", started.as_nanos())
}

fn connection_label(config: &Config, user: Option<&str>, source_index: usize) -> String {
    let user = user.unwrap_or("server-default");
    let ports = config.get_ports();
    let endpoints = if config.get_hosts().is_empty() {
        config
            .get_hostaddrs()
            .iter()
            .enumerate()
            .map(|(index, host)| endpoint(user, &ip_label(*host), port_at(ports, index)))
            .collect::<Vec<_>>()
    } else {
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
    };
    if endpoints.is_empty() {
        format!("{user}@source[{source_index}]")
    } else {
        endpoints.join(",")
    }
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
        host.to_owned()
    } else if host.contains(':') {
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

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
