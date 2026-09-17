//! The two `PostgreSQL` protocol paths used by the collector.
//!
//! Typed metric rows use one-shot unnamed Extended Protocol through
//! [`tokio_postgres::Client::query_typed_raw`]. Small administrative reads may
//! use Simple Protocol when their text representation is unambiguous. Neither
//! path creates named server-side statement state.

mod batch;
mod stats;

pub use batch::read_batched;
pub use stats::{BatchWrite, QueryStats};

use std::collections::HashMap;
use std::pin::pin;
use std::time::Duration;
use std::{error::Error, fmt};

use anyhow::{Context as _, Result};
use futures_util::TryStreamExt as _;
use tokio_postgres::types::{BorrowToSql, FromSqlOwned, Type};
use tokio_postgres::{
    CancelToken, Client, NoTls, Row, RowStream, SimpleQueryMessage, SimpleQueryStream,
};

/// Target row count for one collector-side `PostgreSQL` batch.
pub const BATCH_ROWS: usize = 256;

/// Target decoded payload size for one collector-side `PostgreSQL` batch.
pub const BATCH_LOGICAL_BYTES: usize = 512 * 1024;

/// Maximum characters retained from a potentially unbounded text value.
pub const TEXT_PREFIX_CHARS: usize = 65_536;

/// Server-side deadline installed on every `PostgreSQL` monitoring session.
pub const SERVER_STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Client-side backstop for opening and consuming one bounded row stream.
pub const QUERY_FETCH_TIMEOUT: Duration = Duration::from_secs(35);

pub(crate) const SESSION_SETUP_SQL: &str = concat!(
    marked!("SET statement_timeout = '30s'"),
    "; ",
    marked!("SET lock_timeout = '100ms'"),
);

/// Maximum time spent sending a best-effort `PostgreSQL` `CancelRequest`.
pub const CANCEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// Install the query deadline and lock-wait limit on a new monitoring session.
///
/// The 100 ms limit bounds each lock acquisition wait, not how long an acquired
/// lock is held.
///
/// This uses one Simple Query message and waits for `ReadyForQuery` before the
/// session can be exposed to a caller.
///
/// # Errors
///
/// Returns the server, transport, or protocol error from the `SET` command.
pub async fn configure_session(client: &Client) -> Result<(), tokio_postgres::Error> {
    client.batch_execute(SESSION_SETUP_SQL).await
}

/// Whether `PostgreSQL` cancelled the statement, including `statement_timeout`.
#[must_use]
pub fn is_query_cancelled(error: &tokio_postgres::Error) -> bool {
    error.code() == Some(&tokio_postgres::error::SqlState::QUERY_CANCELED)
}

/// One bounded set of owned rows.
#[derive(Debug)]
pub struct Batch<T> {
    /// Owned rows in arrival order.
    pub rows: Vec<T>,
    /// Approximate decoded application payload in the rows.
    pub logical_bytes: usize,
}

#[derive(Debug)]
struct ColumnLookup {
    indexes: HashMap<Box<str>, usize>,
}

impl ColumnLookup {
    fn new(row: &Row) -> Self {
        Self::from_names(row.columns().iter().map(tokio_postgres::Column::name))
    }

    fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let names = names.into_iter();
        let mut indexes = HashMap::with_capacity(names.size_hint().0);
        for (index, name) in names.enumerate() {
            indexes
                .entry(Box::<str>::from(name.as_ref()))
                .or_insert(index);
        }
        Self { indexes }
    }
}

/// One streamed row with query-scoped column-name indexes.
#[derive(Debug, Clone, Copy)]
pub struct IndexedRow<'a> {
    row: &'a Row,
    columns: &'a ColumnLookup,
}

impl IndexedRow<'_> {
    /// Decode a column by name without rescanning the row description.
    ///
    /// # Errors
    /// Returns the standard `tokio-postgres` missing-column or conversion error.
    pub fn try_get<I, T>(&self, name: I) -> Result<T, tokio_postgres::Error>
    where
        I: AsRef<str>,
        T: FromSqlOwned,
    {
        let name = name.as_ref();
        self.columns
            .indexes
            .get(name)
            .map_or_else(|| self.row.try_get(name), |index| self.row.try_get(*index))
    }
}

/// A streamed query failed either at `PostgreSQL` or in its synchronous sink.
#[derive(Debug)]
pub enum BatchError<E> {
    /// The cumulative query/fetch deadline elapsed.
    Timeout,
    /// The `PostgreSQL` protocol or row stream failed.
    PostgreSql(tokio_postgres::Error),
    /// A returned row did not match the source's declared shape.
    Decode(anyhow::Error),
    /// The collector could not consume the current retained batch.
    Sink(E),
}

/// A typed row failed to decode after `PostgreSQL` returned it successfully.
#[derive(Debug)]
pub struct DecodeError(anyhow::Error);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode a PostgreSQL result row: {:#}", self.0)
    }
}

impl Error for DecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// A small-query stream failed at the transport, protocol, or server boundary.
#[derive(Debug)]
pub struct StreamError(tokio_postgres::Error);

impl StreamError {
    /// The underlying `PostgreSQL` error used for failure classification.
    #[must_use]
    pub const fn postgres(&self) -> &tokio_postgres::Error {
        &self.0
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Error for StreamError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// One live `PostgreSQL` connection and its generation.
#[derive(Debug, Clone, Copy)]
pub struct Session<'a> {
    client: &'a Client,
    generation: u64,
    transport: Option<&'a crate::Transport>,
}

impl<'a> Session<'a> {
    /// Wrap a connected client.
    #[must_use]
    pub const fn new(client: &'a Client, generation: u64) -> Self {
        Self {
            client,
            generation,
            transport: None,
        }
    }

    /// Wrap a client with the transport needed for its cancellation requests.
    #[must_use]
    pub const fn with_transport(
        client: &'a Client,
        generation: u64,
        transport: &'a crate::Transport,
    ) -> Self {
        Self {
            client,
            generation,
            transport: Some(transport),
        }
    }

    /// Generation assigned when this connection was opened.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Start a one-shot unnamed typed Extended Protocol query.
    ///
    /// # Errors
    ///
    /// Returns the transport, protocol, or parameter-encoding error.
    pub async fn typed_stream<P, I>(
        self,
        sql: &str,
        params: I,
        parameter_bytes: usize,
        stats: &mut QueryStats,
    ) -> Result<RowStream, tokio_postgres::Error>
    where
        P: BorrowToSql,
        I: IntoIterator<Item = (P, Type)>,
    {
        stats.sent(sql, parameter_bytes);
        self.client.query_typed_raw(sql, params).await
    }

    /// Start a Simple Protocol query for a small administrative response.
    ///
    /// # Errors
    ///
    /// Returns the transport or protocol error.
    pub async fn simple_stream(
        self,
        sql: &str,
        stats: &mut QueryStats,
    ) -> Result<SimpleQueryStream, tokio_postgres::Error> {
        stats.sent(sql, 0);
        self.client.simple_query_raw(sql).await
    }
}

/// Run a query future with a client-side deadline.
///
/// On timeout, this sends one bounded `PostgreSQL` `CancelRequest` while the
/// connection driver is still alive. The caller must close the original
/// connection after receiving the timeout.
///
/// # Errors
///
/// Returns an elapsed-deadline error after the bounded cancellation attempt.
pub async fn timeout<T>(
    session: Session<'_>,
    duration: Duration,
    future: impl Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    timeout_at(session, tokio::time::Instant::now() + duration, future).await
}

async fn timeout_at<T>(
    session: Session<'_>,
    deadline: tokio::time::Instant,
    future: impl Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    match tokio::time::timeout_at(deadline, future).await {
        Ok(value) => Ok(value),
        Err(elapsed) => {
            send_cancel(session.client.cancel_token(), session.transport).await;
            Err(elapsed)
        }
    }
}

async fn send_cancel(token: CancelToken, transport: Option<&crate::Transport>) {
    let _result = tokio::time::timeout(CANCEL_REQUEST_TIMEOUT, async {
        match transport {
            Some(transport) => transport.cancel(token).await,
            None => token.cancel_query(NoTls).await,
        }
    })
    .await;
}

/// Consume a typed query into memory for a known-small result.
///
/// # Errors
///
/// Returns a `PostgreSQL` protocol/decoding error.
pub async fn read_all<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    mut decode: impl FnMut(&Row) -> Result<T>,
) -> Result<Vec<T>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let stream = session
        .typed_stream(sql, params, parameter_bytes, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut rows = Vec::new();
    let mut decode_error = None;
    while let Some(row) = stream.try_next().await.map_err(StreamError)? {
        stats.received(&row);
        // Drain this known-small response before the connection can be reused.
        if decode_error.is_none() {
            match decode(&row) {
                Ok(decoded) => rows.push(decoded),
                Err(error) => decode_error = Some(error),
            }
        }
    }
    if let Some(error) = decode_error {
        return Err(DecodeError(error).into());
    }
    Ok(rows)
}

/// Consume exactly one typed row.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error when the result does not contain
/// exactly one row.
pub async fn read_one<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    decode: impl FnMut(&Row) -> Result<T>,
) -> Result<T>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut rows = read_all(session, sql, params, parameter_bytes, stats, decode).await?;
    anyhow::ensure!(
        rows.len() == 1,
        "PostgreSQL returned {} rows, expected one",
        rows.len()
    );
    Ok(rows.remove(0))
}

/// Consume zero or one typed row.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error when the result contains more than
/// one row.
pub async fn read_optional<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    decode: impl FnMut(&Row) -> Result<T>,
) -> Result<Option<T>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut rows = read_all(session, sql, params, parameter_bytes, stats, decode).await?;
    anyhow::ensure!(
        rows.len() <= 1,
        "PostgreSQL returned {} rows, expected at most one",
        rows.len()
    );
    Ok(rows.pop())
}

/// Read one integer from an exact one-row, one-column Simple response.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error for a missing, repeated, null, or
/// non-integer scalar value.
pub async fn read_simple_i32(
    session: Session<'_>,
    sql: &str,
    stats: &mut QueryStats,
) -> Result<i32> {
    let stream = session
        .simple_stream(sql, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut value = None;
    while let Some(message) = stream.try_next().await.map_err(StreamError)? {
        if let SimpleQueryMessage::Row(row) = message {
            anyhow::ensure!(value.is_none(), "PostgreSQL returned more than one row");
            let text = row.get(0).context("the scalar column was NULL")?;
            stats.rows = stats.rows.saturating_add(1);
            stats.application_payload_from_postgres_bytes = stats
                .application_payload_from_postgres_bytes
                .saturating_add(text.len() as u64);
            value = Some(
                text.parse::<i32>()
                    .context("parse the PostgreSQL integer scalar")?,
            );
        }
    }
    value.context("PostgreSQL returned no scalar row")
}

/// Consume the row messages from one known-small Simple Protocol query.
///
/// # Errors
///
/// Returns a protocol error or the decoder's error.
pub async fn read_simple_rows<T>(
    session: Session<'_>,
    sql: &str,
    stats: &mut QueryStats,
    mut decode: impl FnMut(&tokio_postgres::SimpleQueryRow) -> Result<T>,
) -> Result<Vec<T>> {
    let stream = session
        .simple_stream(sql, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut rows = Vec::new();
    while let Some(message) = stream.try_next().await.map_err(StreamError)? {
        stats.received_simple(&message);
        if let SimpleQueryMessage::Row(row) = message {
            rows.push(decode(&row).map_err(DecodeError)?);
        }
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "tests/query.rs"]
mod tests;
