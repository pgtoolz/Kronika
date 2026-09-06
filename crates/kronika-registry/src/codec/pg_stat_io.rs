//! Type `1_009_001` / `1_009_002`: `pg_stat_io`.
//!
//! Per `(backend_type, object, context)` I/O counters, available from PG16. The
//! view changed non-additively in PG18 (byte counters added, `op_bytes`
//! removed), so the source maps to two layout versions. Counters and timings are
//! nullable: a combination a backend never performs is `NULL`, not `0`.

use crate::{Section, StrId, Ts};

/// Type `1_009_001`: `pg_stat_io` on PG 16-17.
///
/// `object` is `relation` or `temp relation`; `context` is `normal`, `vacuum`,
/// `bulkread`, or `bulkwrite`. Timings are `0.0` unless `track_io_timing` is on,
/// and `None` only for combinations a backend never performs. `op_bytes` is the
/// fixed I/O unit (PG18 replaced it with per-op byte counters).
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_009_001,
    name = "pg_stat_io",
    semantics = snapshot_full,
    sort_key("backend_type", "object", "context", "ts"),
    identity("backend_type", "object", "context")
)]
pub struct PgStatIoV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Backend type performing the I/O.
    #[column(l)]
    pub backend_type: StrId,
    /// I/O object: `relation` or `temp relation`.
    #[column(l)]
    pub object: StrId,
    /// I/O context: `normal`, `vacuum`, `bulkread`, or `bulkwrite`.
    #[column(l)]
    pub context: StrId,
    /// Read operations; `None` where the combination performs none.
    #[column(c, unit = count)]
    pub reads: Option<i64>,
    /// Time spent reading, ms. `None` only where the backend performs no reads;
    /// `0.0` (not `None`) when `track_io_timing` is off.
    #[column(c, unit = milliseconds)]
    pub read_time: Option<f64>,
    /// Write operations.
    #[column(c, unit = count)]
    pub writes: Option<i64>,
    /// Time spent writing, ms.
    #[column(c, unit = milliseconds)]
    pub write_time: Option<f64>,
    /// Writeback operations.
    #[column(c, unit = count)]
    pub writebacks: Option<i64>,
    /// Time spent in writeback, ms.
    #[column(c, unit = milliseconds)]
    pub writeback_time: Option<f64>,
    /// Relation extend operations.
    #[column(c, unit = count)]
    pub extends: Option<i64>,
    /// Time spent extending, ms.
    #[column(c, unit = milliseconds)]
    pub extend_time: Option<f64>,
    /// Bytes per I/O unit (block size), e.g. 8192. A fixed unit size, not a
    /// counter: total bytes = (`reads` + `writes` + `extends`) * `op_bytes`.
    /// Gauge, so downstream never takes a rate of it.
    #[column(g, unit = bytes)]
    pub op_bytes: Option<i64>,
    /// Buffer hits.
    #[column(c, unit = count)]
    pub hits: Option<i64>,
    /// Buffer evictions.
    #[column(c, unit = count)]
    pub evictions: Option<i64>,
    /// Buffer reuses (during bulk I/O).
    #[column(c, unit = count)]
    pub reuses: Option<i64>,
    /// Fsync operations.
    #[column(c, unit = count)]
    pub fsyncs: Option<i64>,
    /// Time spent in fsync, ms.
    #[column(c, unit = milliseconds)]
    pub fsync_time: Option<f64>,
    /// Time of the last `pg_stat_io` reset; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

/// Type `1_009_002`: `pg_stat_io` on PG 18.
///
/// Replaces `op_bytes` with per-operation byte counters (`read_bytes` etc., a
/// `numeric` in the view, stored as `i64`) and adds WAL rows (`object = wal`,
/// `context = init`), whose timings depend on `track_wal_io_timing` rather than
/// `track_io_timing`. Column semantics otherwise match [`PgStatIoV1`].
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_009_002,
    name = "pg_stat_io",
    semantics = snapshot_full,
    sort_key("backend_type", "object", "context", "ts"),
    identity("backend_type", "object", "context")
)]
pub struct PgStatIoV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Backend type performing the I/O.
    #[column(l)]
    pub backend_type: StrId,
    /// I/O object: `relation`, `temp relation`, or `wal`.
    #[column(l)]
    pub object: StrId,
    /// I/O context: `normal`, `vacuum`, `bulkread`, `bulkwrite`, or `init`.
    #[column(l)]
    pub context: StrId,
    /// Read operations; `None` where the combination performs none.
    #[column(c, unit = count)]
    pub reads: Option<i64>,
    /// Bytes read (`numeric` in the view, stored as `i64`).
    #[column(c, unit = bytes)]
    pub read_bytes: Option<i64>,
    /// Time spent reading, ms.
    #[column(c, unit = milliseconds)]
    pub read_time: Option<f64>,
    /// Write operations.
    #[column(c, unit = count)]
    pub writes: Option<i64>,
    /// Bytes written.
    #[column(c, unit = bytes)]
    pub write_bytes: Option<i64>,
    /// Time spent writing, ms.
    #[column(c, unit = milliseconds)]
    pub write_time: Option<f64>,
    /// Writeback operations.
    #[column(c, unit = count)]
    pub writebacks: Option<i64>,
    /// Time spent in writeback, ms; controlled by `track_io_timing` for all rows.
    #[column(c, unit = milliseconds)]
    pub writeback_time: Option<f64>,
    /// Relation extend operations.
    #[column(c, unit = count)]
    pub extends: Option<i64>,
    /// Bytes added by extends.
    #[column(c, unit = bytes)]
    pub extend_bytes: Option<i64>,
    /// Time spent extending, ms.
    #[column(c, unit = milliseconds)]
    pub extend_time: Option<f64>,
    /// Buffer hits.
    #[column(c, unit = count)]
    pub hits: Option<i64>,
    /// Buffer evictions.
    #[column(c, unit = count)]
    pub evictions: Option<i64>,
    /// Buffer reuses (during bulk I/O).
    #[column(c, unit = count)]
    pub reuses: Option<i64>,
    /// Fsync operations.
    #[column(c, unit = count)]
    pub fsyncs: Option<i64>,
    /// Time spent in fsync, ms.
    #[column(c, unit = milliseconds)]
    pub fsync_time: Option<f64>,
    /// Time of the last `pg_stat_io` reset; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
}

#[cfg(test)]
mod tests {
    use super::{PgStatIoV1, PgStatIoV2};
    use crate::{ColumnClass, Section, StrId, Ts};

    fn v1_row(ts: i64, object: u64) -> PgStatIoV1 {
        PgStatIoV1 {
            ts: Ts(ts),
            backend_type: StrId(1),
            object: StrId(object),
            context: StrId(3),
            reads: Some(100),
            read_time: Some(12.5),
            writes: Some(50),
            write_time: Some(3.0),
            writebacks: Some(0),
            writeback_time: None,
            extends: Some(7),
            extend_time: None,
            op_bytes: Some(8192),
            hits: Some(9000),
            evictions: Some(2),
            reuses: None,
            fsyncs: Some(1),
            fsync_time: None,
            stats_reset: Some(Ts(ts - 1000)),
        }
    }

    #[test]
    fn v1_contract_shape_has_op_bytes_without_byte_counters() {
        let c = PgStatIoV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_009_001);
        assert_eq!(c.columns.len(), 19);
        assert_eq!(c.sort_key, ["backend_type", "object", "context", "ts"]);
        assert_eq!(c.identity, ["backend_type", "object", "context"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("backend_type").map(|col| col.nullable),
            Some(false)
        );
        assert_eq!(c.column("reads").map(|col| col.nullable), Some(true));
        assert!(c.column("op_bytes").is_some());
        assert!(c.column("read_bytes").is_none());
        // op_bytes is a fixed block size, never a counter — a rate of it is bogus.
        assert_eq!(
            c.column("op_bytes").map(|col| col.class),
            Some(ColumnClass::Gauge)
        );
    }

    #[test]
    fn v1_roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[v1_row(1_000, 10), v1_row(1_000, 20)]);
    }

    fn v2_row(ts: i64, object: u64) -> PgStatIoV2 {
        PgStatIoV2 {
            ts: Ts(ts),
            backend_type: StrId(1),
            object: StrId(object),
            context: StrId(3),
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
            hits: Some(9000),
            evictions: Some(2),
            reuses: None,
            fsyncs: Some(1),
            fsync_time: None,
            stats_reset: Some(Ts(ts - 1000)),
        }
    }

    #[test]
    fn v2_contract_shape_has_byte_counters_without_op_bytes() {
        let c = PgStatIoV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_009_002);
        assert_eq!(c.columns.len(), 21);
        assert!(c.column("read_bytes").is_some());
        assert!(c.column("op_bytes").is_none());
        assert_eq!(c.column("read_bytes").map(|col| col.nullable), Some(true));
        assert_eq!(c.sort_key, ["backend_type", "object", "context", "ts"]);
        assert_eq!(c.identity, ["backend_type", "object", "context"]);
    }

    #[test]
    fn v2_roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[v2_row(1_000, 10), v2_row(1_000, 20)]);
    }
}
