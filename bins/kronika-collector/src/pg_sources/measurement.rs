//! One observation per query, including cancellation when an in-flight future is dropped.

use super::execution::QUERY_TIMEOUT;
use kronika_source_pg::query;
use std::time::{Duration, Instant};

/// A completed query or failed connection, passed to the collector diagnostics.
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
