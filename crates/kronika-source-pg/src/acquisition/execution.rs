//! Session checks, query completion, and admission of a batch to the WAL.
//!
//! SQL errors may leave the connection usable. Batch decode errors, client
//! timeouts, and failed batch admission close an unfinished stream's connection.

use super::measurement::QueryMeasurement;
use super::{ConnectionObservation, PgBatch, PgObservation};
use crate::{
    Pool, Session,
    query::{self, BatchError, BatchWrite},
    settings::SettingsRow,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The source reader's fetch deadline; encoding and WAL admission are accounted separately.
pub(super) const QUERY_TIMEOUT: Duration = query::QUERY_FETCH_TIMEOUT;

pub(super) fn deliver<'a, E>(
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
pub(super) enum QueryFailure {
    Source,
    Connection,
    ServerTimeout,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryCompletion {
    Complete,
    SourceFailed,
    CapabilityChanged,
    ConnectionFailed,
    ServerTimedOut,
    TimedOut,
}

pub(super) const fn fixed_source_can_continue(completion: QueryCompletion) -> bool {
    matches!(
        completion,
        QueryCompletion::SourceFailed
            | QueryCompletion::CapabilityChanged
            | QueryCompletion::ServerTimedOut
    )
}

pub(super) async fn open_session<'a>(
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

pub(super) fn session_for_generation<'a>(
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

pub(super) fn finish_query<T>(
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

pub(super) fn finish_failed<T>(
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

pub(super) fn finish_batched<E>(
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

pub(super) fn finish_batched_kind<E>(
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

pub(super) fn capability_sqlstate(code: &str) -> bool {
    matches!(
        code,
        "42P01" | "42883" | "42704" | "42703" | "3F000" | "42501"
    )
}

pub(super) fn postgres_connection_error(error: &anyhow::Error) -> bool {
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
