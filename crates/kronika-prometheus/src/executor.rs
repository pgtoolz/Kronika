//! Executor boundary between the engine and `PostgreSQL` connections.
//!
//! One executor owns the dedicated exporter connection of one database:
//! connect, the one-statement setup probe (server version and recovery
//! role), and catalog metric SQL with column kinds from the vendored
//! driver's type OIDs. Connection-level failures carry
//! `connection_lost` so the engine can derive `instance_up`.

use std::fmt;

use crate::measurement::QueryResult;

/// Server facts learned by the setup probe on a new connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerFacts {
    /// Connected server major version, drives catalog SQL selection.
    pub server_major_version: u32,
    /// `pg_is_in_recovery()` at connect time, drives `node_status` filters.
    pub in_recovery: bool,
}

/// Failure of the setup probe or a metric query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricError {
    /// Five-character SQLSTATE when the server reported one.
    pub sqlstate: Option<String>,
    /// Whether the connection is unusable afterwards (transport, protocol,
    /// or the one-deadline expiry). Drives derived `instance_up`.
    pub connection_lost: bool,
    /// Human-readable error text.
    pub message: String,
}

impl MetricError {
    /// A connection-level failure without a SQLSTATE.
    #[must_use]
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            sqlstate: None,
            connection_lost: true,
            message: message.into(),
        }
    }

    /// Whether this error disables the metric until the next discovery cycle
    /// (missing relation `42P01` or missing function `42883`).
    #[must_use]
    pub fn missing_object(&self) -> bool {
        matches!(self.sqlstate.as_deref(), Some("42P01" | "42883"))
    }
}

impl fmt::Display for MetricError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.sqlstate {
            Some(state) => write!(f, "{}: {}", state, self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for MetricError {}

/// The result of one metric query exchange.
#[derive(Debug)]
pub struct QueryOutcome {
    /// The exchange outcome: text-protocol rows with typed columns, or the
    /// failure. Facts ride both paths — a replaced connection must never
    /// leave the engine with the previous server's facts, even when the
    /// first statement on it fails.
    pub result: Result<QueryResult, MetricError>,
    /// Facts read during the exchange because the underlying connection
    /// was replaced since they were last reported; `None` while the
    /// connection is the one the facts belong to.
    pub refreshed_facts: Option<ServerFacts>,
}

/// The one executor per database: setup probe and metric SQL.
pub trait SqlExecutor {
    /// Connects when needed and runs the setup probe, returning the server
    /// facts. Serves as the availability probe when a database's facts are
    /// not yet known (fresh executor or dropped connection).
    fn server_facts(&mut self) -> impl Future<Output = Result<ServerFacts, MetricError>> + Send;

    /// Runs one single-statement simple-protocol message under the client
    /// deadline (`statement_timeout_s`, or the 5 s default when `None`,
    /// plus the guard margin) and returns the exchange outcome. When the
    /// exchange had to open a replacement connection, the setup probe runs
    /// first under the same deadline and the fresh facts ride the outcome
    /// on success and failure alike.
    fn execute(
        &mut self,
        sql: &str,
        statement_timeout_s: Option<u64>,
    ) -> impl Future<Output = QueryOutcome> + Send;
}

/// Creates the per-database executor when a database appears in discovery.
pub trait ExecutorFactory: Send {
    /// SQL capability for one database.
    type Sql: SqlExecutor;

    /// Builds the executor for `dbname`, or `None` when it cannot be opened.
    fn open_sql(&mut self, dbname: &str) -> impl Send + Future<Output = Option<Self::Sql>>;
}

/// Name of the derived availability row, also used as its cache key.
pub const INSTANCE_UP_METRIC: &str = "instance_up";
