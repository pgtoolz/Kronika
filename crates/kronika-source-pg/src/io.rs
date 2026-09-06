//! `pg_stat_io` collection for types `1_009_001` / `1_009_002`.
//!
//! The view exists from PG16; on PG 10-15 collection emits no batches. PG18
//! changed the schema non-additively (byte counters added, `op_bytes` removed),
//! so the major version selects both the SQL and the layout. Collection streams
//! bounded batches of raw owned rows; the caller interns the label strings. The
//! typed layout lives in `kronika-registry` (`PgStatIoV1` / `PgStatIoV2`).

use kronika_registry::pg_stat_io::{PgStatIoV1, PgStatIoV2};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};

/// The `pg_stat_io` layout selected by the server major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoVersion {
    /// PG 16-17: type `1_009_001` (`op_bytes`, no byte counters).
    V1,
    /// PG 18: type `1_009_002` (byte counters, no `op_bytes`).
    V2,
}

/// Select the layout for a server major version, or `None` before PG16 where
/// `pg_stat_io` does not exist.
#[must_use]
pub const fn io_version(major: u32) -> Option<IoVersion> {
    if major >= 18 {
        Some(IoVersion::V2)
    } else if major >= 16 {
        Some(IoVersion::V1)
    } else {
        None
    }
}

/// The SQL for one layout.
///
/// Each query carries the kronika marker. `ts` is one `statement_timestamp()`
/// for the whole snapshot; `stats_reset` comes back as unix microseconds and the
/// PG18 byte counters (`numeric`) are cast to `int8`.
#[must_use]
pub const fn io_query(version: IoVersion) -> &'static str {
    match version {
        IoVersion::V1 => marked!(
            "SELECT backend_type, object, context, \
             reads, read_time, writes, write_time, \
             writebacks, writeback_time, extends, extend_time, \
             op_bytes, hits, evictions, reuses, fsyncs, fsync_time, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_io"
        ),
        IoVersion::V2 => marked!(
            "SELECT backend_type, object, context, \
             reads, read_bytes::int8 AS read_bytes, read_time, \
             writes, write_bytes::int8 AS write_bytes, write_time, \
             writebacks, writeback_time, \
             extends, extend_bytes::int8 AS extend_bytes, extend_time, \
             hits, evictions, reuses, fsyncs, fsync_time, \
             (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_io"
        ),
    }
}

/// One raw `pg_stat_io` row, a version-agnostic superset. Label strings are
/// owned; the caller interns them. Counters absent from the version are `None`.
/// See [`PgStatIoV1`] / [`PgStatIoV2`] for meaning.
#[derive(Debug, Clone)]
pub struct IoRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Backend type.
    pub backend_type: String,
    /// I/O object.
    pub object: String,
    /// I/O context.
    pub context: String,
    /// Read operations.
    pub reads: Option<i64>,
    /// Bytes read (V2 only).
    pub read_bytes: Option<i64>,
    /// Read time, ms.
    pub read_time: Option<f64>,
    /// Write operations.
    pub writes: Option<i64>,
    /// Bytes written (V2 only).
    pub write_bytes: Option<i64>,
    /// Write time, ms.
    pub write_time: Option<f64>,
    /// Writeback operations.
    pub writebacks: Option<i64>,
    /// Writeback time, ms.
    pub writeback_time: Option<f64>,
    /// Extend operations.
    pub extends: Option<i64>,
    /// Bytes added by extends (V2 only).
    pub extend_bytes: Option<i64>,
    /// Extend time, ms.
    pub extend_time: Option<f64>,
    /// I/O unit size (V1 only).
    pub op_bytes: Option<i64>,
    /// Buffer hits.
    pub hits: Option<i64>,
    /// Buffer evictions.
    pub evictions: Option<i64>,
    /// Buffer reuses.
    pub reuses: Option<i64>,
    /// Fsync operations.
    pub fsyncs: Option<i64>,
    /// Fsync time, ms.
    pub fsync_time: Option<f64>,
    /// Last reset, unix microseconds.
    pub stats_reset: Option<i64>,
}

/// Build a `1_009_001` row (PG16-17 layout, with `op_bytes`), interning labels.
///
/// # Errors
/// Returns the interner's error if a label cannot be interned.
pub fn to_v1<E>(
    row: &IoRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatIoV1, E> {
    Ok(PgStatIoV1 {
        ts: Ts(row.ts),
        backend_type: intern(row.backend_type.as_bytes())?,
        object: intern(row.object.as_bytes())?,
        context: intern(row.context.as_bytes())?,
        reads: row.reads,
        read_time: row.read_time,
        writes: row.writes,
        write_time: row.write_time,
        writebacks: row.writebacks,
        writeback_time: row.writeback_time,
        extends: row.extends,
        extend_time: row.extend_time,
        op_bytes: row.op_bytes,
        hits: row.hits,
        evictions: row.evictions,
        reuses: row.reuses,
        fsyncs: row.fsyncs,
        fsync_time: row.fsync_time,
        stats_reset: row.stats_reset.map(Ts),
    })
}

/// Build a `1_009_002` row (PG18 layout, byte counters), interning labels.
///
/// # Errors
/// Returns the interner's error if a label cannot be interned.
pub fn to_v2<E>(
    row: &IoRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatIoV2, E> {
    Ok(PgStatIoV2 {
        ts: Ts(row.ts),
        backend_type: intern(row.backend_type.as_bytes())?,
        object: intern(row.object.as_bytes())?,
        context: intern(row.context.as_bytes())?,
        reads: row.reads,
        read_bytes: row.read_bytes,
        read_time: row.read_time,
        writes: row.writes,
        write_bytes: row.write_bytes,
        write_time: row.write_time,
        writebacks: row.writebacks,
        writeback_time: row.writeback_time,
        extends: row.extends,
        extend_bytes: row.extend_bytes,
        extend_time: row.extend_time,
        hits: row.hits,
        evictions: row.evictions,
        reuses: row.reuses,
        fsyncs: row.fsyncs,
        fsync_time: row.fsync_time,
        stats_reset: row.stats_reset.map(Ts),
    })
}

/// Read a raw row from a result row using the version's column set.
fn row_from_pg(row: query::IndexedRow<'_>, version: IoVersion) -> anyhow::Result<IoRow> {
    let is_v2 = matches!(version, IoVersion::V2);
    Ok(IoRow {
        ts: row.try_get("ts_us")?,
        backend_type: row.try_get("backend_type")?,
        object: row.try_get("object")?,
        context: row.try_get("context")?,
        reads: row.try_get("reads")?,
        read_bytes: if is_v2 {
            row.try_get("read_bytes")?
        } else {
            None
        },
        read_time: row.try_get("read_time")?,
        writes: row.try_get("writes")?,
        write_bytes: if is_v2 {
            row.try_get("write_bytes")?
        } else {
            None
        },
        write_time: row.try_get("write_time")?,
        writebacks: row.try_get("writebacks")?,
        writeback_time: row.try_get("writeback_time")?,
        extends: row.try_get("extends")?,
        extend_bytes: if is_v2 {
            row.try_get("extend_bytes")?
        } else {
            None
        },
        extend_time: row.try_get("extend_time")?,
        op_bytes: if is_v2 {
            None
        } else {
            row.try_get("op_bytes")?
        },
        hits: row.try_get("hits")?,
        evictions: row.try_get("evictions")?,
        reuses: row.try_get("reuses")?,
        fsyncs: row.try_get("fsyncs")?,
        fsync_time: row.try_get("fsync_time")?,
        stats_reset: row.try_get("stats_reset_us")?,
    })
}

/// Stream a full `pg_stat_io` snapshot in bounded batches. Before PG16, where
/// the view does not exist, this returns successfully without calling `sink`.
///
/// # Errors
/// Returns the `PostgreSQL` stream error or the batch sink error.
pub async fn collect_io<E>(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<IoRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let Some(version) = io_version(major) else {
        return Ok(());
    };
    query::read_batched(
        session,
        io_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| row_from_pg(row, version),
        |_row| 0,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{IoRow, IoVersion, io_query, io_version, to_v1, to_v2};
    use crate::test_intern as fake_intern;

    fn sample_row() -> IoRow {
        IoRow {
            ts: 2_000,
            backend_type: "client backend".to_owned(),
            object: "relation".to_owned(),
            context: "normal".to_owned(),
            reads: Some(100),
            read_bytes: Some(819_200),
            read_time: Some(12.5),
            writes: Some(50),
            write_bytes: Some(409_600),
            write_time: Some(3.0),
            writebacks: Some(0),
            writeback_time: None,
            extends: Some(7),
            extend_bytes: Some(57_344),
            extend_time: None,
            op_bytes: Some(8192),
            hits: Some(9000),
            evictions: Some(2),
            reuses: None,
            fsyncs: Some(1),
            fsync_time: None,
            stats_reset: Some(1_000),
        }
    }

    #[test]
    fn version_appears_at_pg16_and_changes_at_pg18() {
        assert_eq!(io_version(10), None);
        assert_eq!(io_version(15), None);
        assert_eq!(io_version(16), Some(IoVersion::V1));
        assert_eq!(io_version(17), Some(IoVersion::V1));
        assert_eq!(io_version(18), Some(IoVersion::V2));
    }

    #[test]
    fn query_includes_version_specific_columns() {
        assert!(io_query(IoVersion::V1).contains("op_bytes"));
        assert!(!io_query(IoVersion::V1).contains("read_bytes"));
        assert!(io_query(IoVersion::V2).contains("read_bytes"));
        assert!(!io_query(IoVersion::V2).contains("op_bytes"));
        for v in [IoVersion::V1, IoVersion::V2] {
            assert!(io_query(v).contains("pg_stat_io"));
            assert!(io_query(v).contains("kronika:"));
        }
    }

    #[test]
    fn to_v1_interns_labels_and_keeps_op_bytes() {
        let r = to_v1(&sample_row(), fake_intern).expect("intern");
        assert_eq!(r.backend_type, fake_intern(b"client backend").unwrap());
        assert_eq!(r.object, fake_intern(b"relation").unwrap());
        assert_eq!(r.op_bytes, Some(8192));
        assert_eq!(r.reuses, None);
        assert_eq!(r.read_time, Some(12.5));
    }

    #[test]
    fn to_v2_interns_labels_and_keeps_byte_counters() {
        let r = to_v2(&sample_row(), fake_intern).expect("intern");
        assert_eq!(r.context, fake_intern(b"normal").unwrap());
        assert_eq!(r.read_bytes, Some(819_200));
        assert_eq!(r.write_bytes, Some(409_600));
        assert_eq!(r.fsync_time, None);
    }

    #[test]
    fn intern_failure_propagates() {
        assert_eq!(to_v1(&sample_row(), |_| Err("full")), Err("full"));
    }
}
