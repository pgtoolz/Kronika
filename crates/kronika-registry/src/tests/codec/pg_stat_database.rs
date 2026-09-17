use super::{PgStatDatabaseV1, PgStatDatabaseV2, PgStatDatabaseV3, PgStatDatabaseV4};
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn assert_checksum_contract(c: crate::TypeContract) {
    assert_eq!(
        c.column("checksum_failures").map(|column| column.nullable),
        Some(true)
    );
    assert_eq!(
        c.column("checksum_last_failure")
            .and_then(|column| column.unit),
        Some(Unit::Microseconds)
    );
}

fn v4_row(ts: i64, datid: u32) -> PgStatDatabaseV4 {
    PgStatDatabaseV4 {
        ts: Ts(ts),
        datid,
        datname: if datid == 0 {
            None
        } else {
            Some(StrId(datid.into()))
        },
        numbackends: if datid == 0 { Some(0) } else { Some(3) },
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
        stats_reset: Some(Ts(ts - 5)),
        checksum_failures: Some(0),
        checksum_last_failure: None,
        session_time: 1_000.0,
        active_time: 250.0,
        idle_in_transaction_time: 50.0,
        sessions: 7,
        sessions_abandoned: 1,
        sessions_fatal: 0,
        sessions_killed: 0,
        parallel_workers_to_launch: 9,
        parallel_workers_launched: 8,
        frozen_xid_age: if datid == 0 { None } else { Some(150_000_000) },
        min_mxid_age: if datid == 0 { None } else { Some(5_000_000) },
        datconnlimit: if datid == 0 { None } else { Some(-1) },
        datallowconn: if datid == 0 { None } else { Some(true) },
        datistemplate: if datid == 0 { None } else { Some(false) },
    }
}

#[test]
fn v4_contract_shape_matches_the_registry() {
    let c = PgStatDatabaseV4::CONTRACT;
    assert_eq!(c.type_id.get(), 1_005_004);
    assert_eq!(c.columns.len(), 36);
    assert_eq!(c.sort_key, ["datid", "ts"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("datid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("datname").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("numbackends").map(|col| col.nullable), Some(true));
    assert_eq!(
        c.column("checksum_last_failure").map(|col| col.nullable),
        Some(true)
    );
    assert!(c.column("parallel_workers_launched").is_some());
    assert!(c.column("session_time").is_some());
    assert_eq!(
        c.column("frozen_xid_age").map(|col| col.nullable),
        Some(true)
    );
    assert_eq!(c.column("datconnlimit").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("datallowconn").map(|col| col.nullable), Some(true));
    assert_checksum_contract(c);
}

#[test]
fn v4_roundtrip_preserves_values_and_nulls() {
    let mut checksum_disabled = v4_row(5, 1);
    checksum_disabled.checksum_failures = None;
    // The shared-objects row sorts before database rows.
    crate::assert_roundtrips(&[v4_row(1_000, 0), checksum_disabled, v4_row(1_000, 5)]);
}

#[test]
fn v4_encode_sorts_by_datid_then_ts() {
    let bytes = PgStatDatabaseV4::encode(&[v4_row(1_000, 9), v4_row(1_000, 1)]).expect("encode");
    let decoded =
        PgStatDatabaseV4::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded.iter().map(|r| r.datid).collect::<Vec<_>>(), [1, 9]);
}

fn v3_row(ts: i64, datid: u32) -> PgStatDatabaseV3 {
    PgStatDatabaseV3 {
        ts: Ts(ts),
        datid,
        datname: Some(StrId(1)),
        numbackends: Some(3),
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
        stats_reset: Some(Ts(ts - 5)),
        checksum_failures: Some(0),
        checksum_last_failure: Some(Ts(ts - 1)),
        session_time: 1_000.0,
        active_time: 250.0,
        idle_in_transaction_time: 50.0,
        sessions: 7,
        sessions_abandoned: 1,
        sessions_fatal: 0,
        sessions_killed: 0,
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

#[test]
fn v3_contract_has_session_without_parallel() {
    let c = PgStatDatabaseV3::CONTRACT;
    assert_eq!(c.type_id.get(), 1_005_003);
    assert_eq!(c.columns.len(), 34);
    assert!(c.column("session_time").is_some());
    assert!(c.column("parallel_workers_launched").is_none());
    assert_checksum_contract(c);
}

#[test]
fn v3_roundtrip() {
    let mut checksum_disabled = v3_row(1_000, 2);
    checksum_disabled.checksum_failures = None;
    crate::assert_roundtrips(&[v3_row(1_000, 1), checksum_disabled]);
}

fn v2_row(ts: i64, datid: u32) -> PgStatDatabaseV2 {
    PgStatDatabaseV2 {
        ts: Ts(ts),
        datid,
        datname: Some(StrId(1)),
        numbackends: Some(3),
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
        stats_reset: Some(Ts(ts - 5)),
        checksum_failures: Some(0),
        checksum_last_failure: None,
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

#[test]
fn v2_contract_has_checksum_without_session() {
    let c = PgStatDatabaseV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_005_002);
    assert_eq!(c.columns.len(), 27);
    assert!(c.column("checksum_failures").is_some());
    assert!(c.column("session_time").is_none());
    assert_checksum_contract(c);
}

#[test]
fn v2_roundtrip() {
    let mut checksum_disabled = v2_row(1_000, 2);
    checksum_disabled.checksum_failures = None;
    crate::assert_roundtrips(&[v2_row(1_000, 1), checksum_disabled]);
}

fn v1_row(ts: i64, datid: u32) -> PgStatDatabaseV1 {
    PgStatDatabaseV1 {
        ts: Ts(ts),
        datid,
        datname: Some(StrId(1)),
        numbackends: Some(3),
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
        stats_reset: Some(Ts(ts - 5)),
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

#[test]
fn v1_contract_is_the_base_layout() {
    let c = PgStatDatabaseV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_005_001);
    assert_eq!(c.columns.len(), 25);
    assert!(c.column("checksum_failures").is_none());
    assert!(c.column("session_time").is_none());
}

#[test]
fn v1_roundtrip() {
    crate::assert_roundtrips(&[v1_row(1_000, 1), v1_row(1_000, 2)]);
}
