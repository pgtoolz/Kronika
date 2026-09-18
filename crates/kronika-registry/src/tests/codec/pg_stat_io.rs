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
