//! Approximate application payload and journal timing, independent of wire framing.

use std::time::Duration;
use tokio_postgres::{Row, SimpleQueryMessage, types::Type};

/// Measurements made while one SQL statement is consumed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QueryStats {
    /// Result rows decoded from `PostgreSQL`.
    pub rows: u64,
    /// Approximate decoded application payload, not network or TLS bytes.
    pub application_payload_from_postgres_bytes: u64,
    /// Query text and parameter payload supplied by the application, not wire bytes.
    pub application_payload_to_postgres_bytes: u64,
    /// Batches handed to the collector before another row was fetched.
    pub batches: u64,
    /// Time spent encoding batches for the journal.
    pub encode_elapsed: Duration,
    /// Time spent appending encoded batches to the journal.
    pub append_elapsed: Duration,
    /// Encoded part bytes attributable to this query.
    pub encoded_bytes: u64,
    /// Bytes appended to the WAL attributable to this query.
    pub wal_bytes_appended: u64,
    sink_elapsed: Duration,
}

impl QueryStats {
    /// Account for a query before sending it.
    pub(super) const fn sent(&mut self, sql: &str, parameter_bytes: usize) {
        self.application_payload_to_postgres_bytes = self
            .application_payload_to_postgres_bytes
            .saturating_add(sql.len().saturating_add(parameter_bytes) as u64);
    }

    /// Database fetch time with synchronous encoding and append time removed.
    #[must_use]
    pub const fn fetch_elapsed(self, total: Duration) -> Duration {
        total.saturating_sub(self.sink_elapsed)
    }

    /// Account for one batch encoded and appended outside the stream helper.
    pub const fn record_batch_write(&mut self, elapsed: Duration, write: BatchWrite) {
        self.batches = self.batches.saturating_add(1);
        self.sink_elapsed = self.sink_elapsed.saturating_add(elapsed);
        self.encode_elapsed = self.encode_elapsed.saturating_add(write.encode_elapsed);
        self.append_elapsed = self.append_elapsed.saturating_add(write.append_elapsed);
        self.encoded_bytes = self.encoded_bytes.saturating_add(write.encoded_bytes);
        self.wal_bytes_appended = self
            .wal_bytes_appended
            .saturating_add(write.wal_bytes_appended);
    }

    /// Account for batch sink work that failed before bytes were written.
    pub const fn record_failed_batch(&mut self, elapsed: Duration) {
        self.batches = self.batches.saturating_add(1);
        self.sink_elapsed = self.sink_elapsed.saturating_add(elapsed);
    }

    /// Account for one message returned by Simple Protocol.
    pub fn received_simple(&mut self, message: &SimpleQueryMessage) {
        let SimpleQueryMessage::Row(row) = message else {
            return;
        };
        self.rows = self.rows.saturating_add(1);
        let bytes = (0..row.len())
            .filter_map(|index| row.get(index))
            .map(str::len)
            .fold(0_usize, usize::saturating_add);
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(bytes as u64);
    }

    pub(super) fn received(&mut self, row: &Row) -> usize {
        let bytes = logical_row_bytes(row);
        self.rows = self.rows.saturating_add(1);
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(bytes as u64);
        bytes
    }

    pub(super) const fn add_decoded_payload(
        &mut self,
        row_bytes: usize,
        decoded_bytes: usize,
    ) -> usize {
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(decoded_bytes as u64);
        row_bytes.saturating_add(decoded_bytes)
    }
}

/// Journal work attributable to one `PostgreSQL` batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchWrite {
    /// Time spent converting and encoding the batch.
    pub encode_elapsed: Duration,
    /// Time spent appending the encoded part.
    pub append_elapsed: Duration,
    /// Encoded part bytes.
    pub encoded_bytes: u64,
    /// Bytes added to the WAL.
    pub wal_bytes_appended: u64,
}

fn logical_row_bytes(row: &Row) -> usize {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let ty = column.type_();
            if matches!(
                *ty,
                Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN
            ) {
                return row
                    .try_get::<_, Option<&str>>(index)
                    .ok()
                    .flatten()
                    .map_or(0, str::len);
            }
            if *ty == Type::BYTEA {
                return row
                    .try_get::<_, Option<&[u8]>>(index)
                    .ok()
                    .flatten()
                    .map_or(0, <[u8]>::len);
            }
            fixed_width(ty)
        })
        .fold(0_usize, usize::saturating_add)
}

fn fixed_width(ty: &Type) -> usize {
    if matches!(*ty, Type::BOOL | Type::CHAR) {
        1
    } else if *ty == Type::INT2 {
        2
    } else if matches!(*ty, Type::INT4 | Type::OID | Type::FLOAT4 | Type::DATE) {
        4
    } else if matches!(
        *ty,
        Type::INT8 | Type::FLOAT8 | Type::TIME | Type::TIMESTAMP | Type::TIMESTAMPTZ
    ) {
        8
    } else {
        0
    }
}
