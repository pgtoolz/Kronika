use crate::test_intern as intern;

use super::{
    LockRow, LocksVersion, blocked_by_logical_bytes, locks_query, locks_version, to_v1, to_v2,
};

fn row() -> LockRow {
    LockRow {
        ts: 2_000_000,
        pid: 20,
        blocked_by: vec![10, 0],
        datid: 16_384,
        datname: "app".to_owned(),
        usename: Some("monitor".to_owned()),
        application_name: "psql".to_owned(),
        client_addr: "127.0.0.1".to_owned(),
        backend_type: "client backend".to_owned(),
        state: Some("active".to_owned()),
        wait_event_type: Some("Lock".to_owned()),
        wait_event: Some("transactionid".to_owned()),
        query: "update t set v = 1".to_owned(),
        backend_xid_age: Some(5),
        backend_xmin_age: Some(3),
        backend_start: Some(1_000_000),
        xact_start: Some(1_500_000),
        query_start: Some(1_750_000),
        state_change: Some(1_750_000),
        lock_locktype: Some("transactionid".to_owned()),
        lock_mode: Some("ShareLock".to_owned()),
        lock_database: Some(16_384),
        lock_relation: None,
        lock_relname: None,
        lock_page: None,
        lock_tuple: None,
        lock_virtualxid: None,
        lock_transactionid: Some(42),
        lock_classid: None,
        lock_objid: None,
        lock_objsubid: None,
        lock_target: Some("transaction 42".to_owned()),
        waitstart: Some(1_900_000),
    }
}

#[test]
fn version_changes_with_waitstart() {
    assert_eq!(locks_version(10), LocksVersion::V1);
    assert_eq!(locks_version(13), LocksVersion::V1);
    assert_eq!(locks_version(14), LocksVersion::V2);
    assert_eq!(locks_version(18), LocksVersion::V2);
}

#[test]
fn query_builds_a_bounded_backend_graph() {
    let v1 = locks_query(LocksVersion::V1);
    let v2 = locks_query(LocksVersion::V2);
    assert!(v1.contains("pg_blocking_pids"));
    assert_eq!(v1.matches("pg_blocking_pids").count(), 1);
    assert!(v1.contains("NOT l.granted"));
    assert!(v1.contains("l.pid <> pg_catalog.pg_backend_pid()"));
    assert_eq!(v1.matches("current_setting('application_name')").count(), 2);
    assert!(v1.contains("own.application_name IS NOT DISTINCT FROM"));
    assert!(v1.contains("SELECT DISTINCT ON (l.pid)"));
    assert!(v1.contains("l.transactionid::text"));
    assert!(!v1.contains("WITH RECURSIVE"));
    assert!(!v1.contains("root_pid"));
    assert!(!v1.contains("depth"));
    assert!(!v1.contains("waitstart_us"));
    assert!(v2.contains("waitstart_us"));
    assert!(v2.contains("kronika:"));
}

#[test]
fn converters_preserve_edges_and_versioned_waitstart() {
    let raw = row();
    let v1 = to_v1(&raw, intern).expect("infallible intern");
    let v2 = to_v2(&raw, intern).expect("infallible intern");
    assert_eq!(v1.blocked_by, [10, 0]);
    assert_eq!(v1.lock_transactionid, Some(42));
    assert_eq!(v2.blocked_by, [10, 0]);
    assert_eq!(v2.waitstart.map(|ts| ts.0), Some(1_900_000));
}

#[test]
fn blocked_by_arrays_contribute_their_decoded_bytes() {
    assert_eq!(blocked_by_logical_bytes(&row()), 2 * size_of::<i32>());
}
