use super::PgStorePlansDatasentinelV1;
use crate::{Section, StrId, Ts, Unit};

fn row(calls: i64) -> PgStorePlansDatasentinelV1 {
    PgStorePlansDatasentinelV1 {
        ts: Ts(2_000),
        queryid: -7,
        planid: 991,
        userid: 10,
        dbid: 16_400,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        plan: Some(StrId(3)),
        relids: Some(StrId(4)),
        cmd_type: Some(StrId(5)),
        calls,
        total_time: 99.5,
        min_time: 1.0,
        max_time: 50.0,
        mean_time: 24.9,
        stddev_time: 2.2,
        rows: 40,
        shared_blks_hit: 1,
        shared_blks_read: 2,
        shared_blks_dirtied: 3,
        shared_blks_written: 4,
        local_blks_hit: 5,
        local_blks_read: 6,
        local_blks_dirtied: 7,
        local_blks_written: 8,
        temp_blks_read: 9,
        temp_blks_written: 10,
        shared_blk_read_time: 1.5,
        shared_blk_write_time: 2.5,
        local_blk_read_time: 3.5,
        local_blk_write_time: 4.5,
        temp_blk_read_time: 5.5,
        temp_blk_write_time: 6.5,
        first_call: (calls > 0).then_some(Ts(1_000)),
        last_call: (calls > 0).then_some(Ts(1_900)),
    }
}

#[test]
fn datasentinel_contract_keeps_its_distinct_shape() {
    let contract = PgStorePlansDatasentinelV1::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_018_001);
    assert_eq!(contract.columns.len(), 35);
    assert_eq!(contract.identity, ["userid", "dbid", "queryid", "planid"]);
    assert_eq!(contract.sort_key, ["dbid", "userid", "queryid", "planid"]);
    assert_eq!(
        contract.column("relids").map(|col| col.nullable),
        Some(true)
    );
    assert_eq!(
        contract.column("cmd_type").map(|col| col.nullable),
        Some(true)
    );
    for name in ["first_call", "last_call"] {
        assert_eq!(contract.column(name).map(|col| col.nullable), Some(true));
        assert_eq!(
            contract.column(name).and_then(|col| col.unit),
            Some(Unit::Microseconds)
        );
    }
}

#[test]
fn datasentinel_roundtrips_completed_and_in_flight_entries() {
    crate::assert_roundtrips(&[row(1), row(0)]);
}
