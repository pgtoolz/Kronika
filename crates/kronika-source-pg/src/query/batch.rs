//! Bounded typed batches with sink time excluded from the fetch deadline.

use anyhow::Result;
use futures_util::TryStreamExt as _;
use std::pin::pin;
use std::time::{Duration, Instant};
use tokio_postgres::types::{BorrowToSql, Type};

use super::{
    BATCH_LOGICAL_BYTES, BATCH_ROWS, Batch, BatchError, BatchWrite, ColumnLookup, IndexedRow,
    QUERY_FETCH_TIMEOUT, QueryStats, Session, timeout_at,
};

/// Decode and deliver bounded batches before fetching more rows.
///
/// `decoded_logical_bytes` accounts for owned payloads that cannot be measured
/// from a borrowed [`tokio_postgres::Row`] without decoding them a second time.
///
/// # Errors
///
/// Returns [`BatchError::Timeout`] when the cumulative query/fetch deadline
/// elapses, [`BatchError::PostgreSql`] for a stream failure,
/// [`BatchError::Decode`] for a row-shape mismatch, and [`BatchError::Sink`]
/// when the synchronous sink rejects a retained batch.
#[allow(
    clippy::too_many_arguments,
    reason = "stream limits, accounting, decoding, and sink ownership stay explicit"
)]
pub async fn read_batched<P, I, T, E>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    mut decode: impl FnMut(IndexedRow<'_>) -> Result<T>,
    mut decoded_logical_bytes: impl FnMut(&T) -> usize,
    mut sink: impl FnMut(Batch<T>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut deadline = tokio::time::Instant::now() + QUERY_FETCH_TIMEOUT;
    let stream = timeout_at(
        session,
        deadline,
        session.typed_stream(sql, params, parameter_bytes, stats),
    )
    .await
    .map_err(|_elapsed| BatchError::Timeout)?
    .map_err(BatchError::PostgreSql)?;
    let mut stream = pin!(stream);
    let mut pending = PendingBatch::new();
    let mut columns = None;
    while let Some(row) = timeout_at(session, deadline, stream.try_next())
        .await
        .map_err(|_elapsed| BatchError::Timeout)?
        .map_err(BatchError::PostgreSql)?
    {
        let row_bytes = stats.received(&row);
        let columns = columns.get_or_insert_with(|| ColumnLookup::new(&row));
        let decoded = decode(IndexedRow { row: &row, columns }).map_err(BatchError::Decode)?;
        let bytes = stats.add_decoded_payload(row_bytes, decoded_logical_bytes(&decoded));
        if let Some(batch) = pending.push(decoded, bytes) {
            extend_fetch_deadline(&mut deadline, deliver_batch(batch, stats, &mut sink)?);
        }
    }
    if let Some(batch) = pending.finish() {
        let _sink_elapsed = deliver_batch(batch, stats, &mut sink)?;
    }
    Ok(())
}

fn extend_fetch_deadline(deadline: &mut tokio::time::Instant, sink_elapsed: Duration) {
    if let Some(extended) = deadline.checked_add(sink_elapsed) {
        *deadline = extended;
    }
}

const fn batch_limit_reached(rows: usize, logical_bytes: usize) -> bool {
    rows >= BATCH_ROWS || logical_bytes >= BATCH_LOGICAL_BYTES
}

struct PendingBatch<T> {
    rows: Vec<T>,
    logical_bytes: usize,
}

impl<T> PendingBatch<T> {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(BATCH_ROWS),
            logical_bytes: 0,
        }
    }

    fn push(&mut self, row: T, logical_bytes: usize) -> Option<Batch<T>> {
        self.logical_bytes = self.logical_bytes.saturating_add(logical_bytes);
        self.rows.push(row);
        batch_limit_reached(self.rows.len(), self.logical_bytes).then(|| self.take())
    }

    fn finish(mut self) -> Option<Batch<T>> {
        (!self.rows.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> Batch<T> {
        Batch {
            rows: std::mem::replace(&mut self.rows, Vec::with_capacity(BATCH_ROWS)),
            logical_bytes: std::mem::take(&mut self.logical_bytes),
        }
    }
}

fn deliver_batch<T, E>(
    batch: Batch<T>,
    stats: &mut QueryStats,
    sink: &mut impl FnMut(Batch<T>) -> Result<BatchWrite, E>,
) -> Result<Duration, BatchError<E>> {
    let started = Instant::now();
    match sink(batch) {
        Ok(write) => {
            let elapsed = started.elapsed();
            stats.record_batch_write(elapsed, write);
            Ok(elapsed)
        }
        Err(error) => {
            stats.record_failed_batch(started.elapsed());
            Err(BatchError::Sink(error))
        }
    }
}

#[cfg(test)]
#[path = "../tests/query/batch.rs"]
mod tests;
