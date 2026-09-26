//! Executor boundary between the engine and `PostgreSQL` connections.
//!
//! The engine drives one ping/SQL pair of capabilities per database. The
//! ping executor owns the dedicated exporter connection: connect, session
//! `SET`s, the availability check and the server handshake. The SQL executor
//! runs catalog metric SQL and resolves result column kinds — the one piece
//! waiting on the tokio-postgres type-OID patch; until that pin lands,
//! collectors pass no SQL executor and the engine serves `instance_up` plus
//! self metrics only.

use std::fmt;

use crate::measurement::QueryResult;

/// Server facts learned at connection time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingInfo {
    /// Connected server major version, drives catalog SQL selection.
    pub server_major_version: u32,
    /// `pg_is_in_recovery()` at connect time, drives `node_status` filters.
    pub in_recovery: bool,
}

/// Failure of a ping or metric query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricError {
    /// Five-character SQLSTATE when the server reported one.
    pub sqlstate: Option<String>,
    /// Whether the connection is unusable afterwards (transport/protocol).
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

/// Availability check plus handshake on the dedicated exporter connection.
///
/// A transport failure here means the database is down for the exporter:
/// `instance_up` becomes 0 until a later ping succeeds.
pub trait PingExecutor {
    /// Runs `SELECT 1`-equivalent work and reports server facts.
    fn ping(&mut self) -> impl Future<Output = Result<PingInfo, MetricError>> + Send;

    /// Fresh `pg_is_in_recovery()` for `node_status` filtering (CAT-8).
    fn recovery_role(&mut self) -> impl Future<Output = Result<bool, MetricError>> + Send;
}

/// Catalog metric SQL execution with column kinds resolved from type OIDs.
pub trait SqlExecutor {
    /// Runs one statement under `statement_timeout_s` (or the 5 s session
    /// default when `None`) and returns text-protocol rows with typed columns.
    fn execute(
        &mut self,
        sql: &str,
        statement_timeout_s: Option<u64>,
    ) -> impl Future<Output = Result<QueryResult, MetricError>> + Send;
}

/// Creates per-database executors.
///
/// `open_ping` is called when a database appears in discovery; `open_sql` is
/// called once per database and may return `None` while the type-OID patch
/// is not pinned — metric SQL collection then stays off.
pub trait ExecutorFactory: Send {
    /// Ping capability for one database.
    type Ping: PingExecutor;
    /// SQL capability for one database when available.
    type Sql: SqlExecutor;

    /// Build the ping executor for `dbname`.
    fn open_ping(&mut self, dbname: &str) -> impl Send + Future<Output = Option<Self::Ping>>;

    /// Build the SQL executor for `dbname`, or `None` while unavailable.
    fn open_sql(&mut self, dbname: &str) -> impl Send + Future<Output = Option<Self::Sql>>;
}
