use super::{
    DatabaseRow, DatabaseVersion, database_query, database_version, to_v1, to_v2, to_v3, to_v4,
};
use crate::tests::intern as fake_intern;

fn sample_row(datid: u32) -> DatabaseRow {
    DatabaseRow {
        ts: 2_000,
        datid,
        datname: if datid == 0 {
            None
        } else {
            Some("appdb".to_owned())
        },
        numbackends: if datid == 0 { Some(0) } else { Some(4) },
        xact_commit: 100,
        xact_rollback: 2,
        blks_read: 4_000,
        blks_hit: 90_000,
        tup_returned: 500,
        tup_fetched: 400,
        tup_inserted: 50,
        tup_updated: 30,
        tup_deleted: 10,
        conflicts: 0,
        temp_files: 1,
        temp_bytes: 8_192,
        deadlocks: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        stats_reset: Some(1_500),
        checksum_failures: Some(0),
        checksum_last_failure: None,
        session_time: Some(1_000.0),
        active_time: Some(250.0),
        idle_in_transaction_time: Some(50.0),
        sessions: Some(7),
        sessions_abandoned: Some(1),
        sessions_fatal: Some(0),
        sessions_killed: Some(0),
        parallel_workers_to_launch: Some(9),
        parallel_workers_launched: Some(8),
        frozen_xid_age: if datid == 0 { None } else { Some(150_000_000) },
        min_mxid_age: if datid == 0 { None } else { Some(5_000_000) },
        datconnlimit: if datid == 0 { None } else { Some(-1) },
        datallowconn: if datid == 0 { None } else { Some(true) },
        datistemplate: if datid == 0 { None } else { Some(false) },
    }
}

#[test]
fn version_follows_catalog_changes() {
    assert_eq!(database_version(10), DatabaseVersion::V1);
    assert_eq!(database_version(11), DatabaseVersion::V1);
    assert_eq!(database_version(12), DatabaseVersion::V2);
    assert_eq!(database_version(13), DatabaseVersion::V2);
    assert_eq!(database_version(14), DatabaseVersion::V3);
    assert_eq!(database_version(17), DatabaseVersion::V3);
    assert_eq!(database_version(18), DatabaseVersion::V4);
}

#[test]
fn query_includes_version_specific_columns() {
    assert!(!database_query(DatabaseVersion::V1).contains("checksum_failures"));
    assert!(database_query(DatabaseVersion::V2).contains("checksum_failures"));
    assert!(!database_query(DatabaseVersion::V2).contains("session_time"));
    assert!(database_query(DatabaseVersion::V3).contains("session_time"));
    assert!(!database_query(DatabaseVersion::V3).contains("parallel_workers_launched"));
    assert!(database_query(DatabaseVersion::V4).contains("parallel_workers_launched"));
    for v in [
        DatabaseVersion::V1,
        DatabaseVersion::V2,
        DatabaseVersion::V3,
        DatabaseVersion::V4,
    ] {
        assert!(database_query(v).contains("pg_stat_database"));
        assert!(database_query(v).contains("kronika:"));
        assert!(database_query(v).contains("frozen_xid_age"));
        assert!(!database_query(v).contains(concat!("pg_database", "_size")));
        assert!(database_query(v).contains("LEFT JOIN pg_database"));
    }
}

#[test]
fn to_v4_maps_every_column_and_interns_datname() {
    let r = to_v4(&sample_row(5), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.datid, 5);
    assert_eq!(r.datname, Some(fake_intern(b"appdb").unwrap()));
    assert_eq!(r.numbackends, Some(4));
    assert!((r.blk_read_time - 12.5).abs() < f64::EPSILON);
    assert_eq!(r.checksum_failures, Some(0));
    assert_eq!(r.checksum_last_failure, None);
    assert_eq!(r.parallel_workers_launched, 8);
    assert_eq!(r.frozen_xid_age, Some(150_000_000));
    assert_eq!(r.min_mxid_age, Some(5_000_000));
    assert_eq!(r.datconnlimit, Some(-1));
    assert_eq!(r.datallowconn, Some(true));
    assert_eq!(r.datistemplate, Some(false));
}

#[test]
fn to_v4_shared_row_preserves_numbackends_and_null_catalog_fields() {
    let r = to_v4(&sample_row(0), fake_intern).expect("intern");
    assert_eq!(r.datid, 0);
    assert_eq!(r.datname, None);
    assert_eq!(r.numbackends, Some(0));
    assert_eq!(r.frozen_xid_age, None);
    assert_eq!(r.min_mxid_age, None);
    assert_eq!(r.datconnlimit, None);
    assert_eq!(r.datallowconn, None);
    assert_eq!(r.datistemplate, None);
}

#[test]
fn disabled_checksums_stay_null_in_every_checksum_layout() {
    let row = DatabaseRow {
        checksum_failures: None,
        ..sample_row(5)
    };
    assert_eq!(
        to_v2(&row, fake_intern).expect("intern").checksum_failures,
        None
    );
    assert_eq!(
        to_v3(&row, fake_intern).expect("intern").checksum_failures,
        None
    );
    assert_eq!(
        to_v4(&row, fake_intern).expect("intern").checksum_failures,
        None
    );
}

#[test]
fn to_v1_maps_the_base_layout() {
    let r = to_v1(&sample_row(5), fake_intern).expect("intern");
    assert_eq!(r.datid, 5);
    assert_eq!(r.datname, Some(fake_intern(b"appdb").unwrap()));
    assert_eq!(r.xact_commit, 100);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v4(&sample_row(5), |_| Err("full")), Err("full"));
}
