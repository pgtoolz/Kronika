use super::{IoRow, IoVersion, io_query, io_version, to_v1, to_v2};
use crate::tests::intern as fake_intern;

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
