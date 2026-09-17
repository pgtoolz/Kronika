//! One observation per query, including cancellation when an in-flight future is dropped.

use super::execution::QUERY_TIMEOUT;
use crate::query;
use std::time::{Duration, Instant};

/// A completed query or failed connection, passed to the collector diagnostics.
#[derive(Debug)]
pub enum PgObservation {
    /// One completed or interrupted query.
    Query(QueryObservation),
    /// One failed connection attempt.
    Connection(ConnectionObservation),
    /// A source condition requiring operator attention.
    Warning(PgWarning),
}

/// One completed or interrupted SQL statement.
#[derive(Debug)]
pub struct QueryObservation {
    /// Stable identity of the SQL statement.
    pub query_name: &'static str,
    /// Credential-safe endpoint identity.
    pub connection: String,
    /// Resolved database or configured fallback.
    pub database: String,
    /// Total wall time including synchronous admission.
    pub elapsed: Duration,
    /// Fetch and admission accounting.
    pub stats: query::QueryStats,
    /// Completion classification.
    pub outcome: QueryOutcome,
    /// Failure description, when present.
    pub error: Option<String>,
}

/// One failed connection attempt.
#[derive(Debug)]
pub struct ConnectionObservation {
    /// Credential-safe endpoint identity.
    pub connection: String,
    /// Resolved database or configured fallback.
    pub database: String,
    /// Total wall time including synchronous admission.
    pub elapsed: Duration,
    /// Whether the connection deadline elapsed.
    pub timeout: bool,
    /// Whether an existing session closed before query execution.
    pub closed: bool,
    /// Failure description, when present.
    pub error: String,
}

/// Stable query completion classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryOutcome {
    /// The query and all admitted batches completed.
    Success,
    /// The source failed or acquisition was cancelled.
    Error,
    /// The client deadline or server statement timeout elapsed.
    Timeout,
    /// The caller rejected a batch with an error.
    SinkError,
}

/// Source warnings reported separately from completed query accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgWarning {
    /// A discovered statement extension needs an SQL extension upgrade.
    StatementsExtensionUpdateRequired {
        /// Database containing the extension.
        database: String,
        /// Installed extension version.
        extension_version: String,
    },
    /// The login role cannot read complete activity statistics.
    StatsVisibilityRequired {
        /// Database containing the restricted session.
        database: String,
    },
}

pub(super) struct QueryMeasurement<'a> {
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    connection: String,
    database: String,
    started: Instant,
    stats: query::QueryStats,
    finished: bool,
}

impl QueryMeasurement<'_> {
    pub(super) const fn stats_mut(&mut self) -> &mut query::QueryStats {
        &mut self.stats
    }

    pub(super) fn resolve_identity(&mut self, connection: String, database: String) {
        self.connection = connection;
        self.database = database;
    }

    pub(super) fn success(mut self) {
        self.emit(QueryOutcome::Success, None);
    }

    pub(super) fn error(mut self, message: String) {
        self.emit(QueryOutcome::Error, Some(message));
    }

    pub(super) fn timeout(mut self) {
        self.emit(
            QueryOutcome::Timeout,
            Some(format!(
                "query timed out after {} seconds",
                QUERY_TIMEOUT.as_secs()
            )),
        );
    }

    pub(super) fn server_timeout(mut self, message: String) {
        self.emit(QueryOutcome::Timeout, Some(message));
    }

    pub(super) fn sink_error(mut self) {
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

pub(super) fn measure<'a>(
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
