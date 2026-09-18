use super::PgStorePlansVadvV1;
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn row(ts: i64, dbid: u32, userid: u32, queryid: i64, plan: Option<StrId>) -> PgStorePlansVadvV1 {
    PgStorePlansVadvV1 {
        ts: Ts(ts),
        userid,
        dbid,
        queryid,
        planid: -7_000_000_001,
        queryid_stat_statements: 4_242_424_242_424,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        plan,
        calls: 12,
        slow_log_calls: 1,
        total_time: 1_234.5,
        min_time: 0.5,
        max_time: 900.0,
        mean_time: 102.9,
        stddev_time: 3.3,
        rows: 400,
        shared_blks_hit: 10,
        shared_blks_read: 11,
        shared_blks_dirtied: 12,
        shared_blks_written: 13,
        local_blks_hit: 14,
        local_blks_read: 15,
        local_blks_dirtied: 16,
        local_blks_written: 17,
        temp_blks_read: 18,
        temp_blks_written: 19,
        blk_read_time: 20.5,
        blk_write_time: 21.5,
        first_call: Ts(ts - 5_000_000),
        last_call: Ts(ts - 1),
        total_plan_time: 7.5,
        min_plan_time: 0.1,
        max_plan_time: 2.0,
        mean_plan_time: 0.6,
    }
}

#[test]
fn vadv_v1_contract_shape() {
    let c = PgStorePlansVadvV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_004_001);
    assert_eq!(c.columns.len(), 35);
    assert_eq!(c.sort_key, ["dbid", "userid", "queryid", "planid"]);
    assert_eq!(c.identity, ["userid", "dbid", "queryid", "planid"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("queryid").map(|col| col.nullable), Some(false));
    assert_eq!(
        c.column("queryid_stat_statements").map(|col| col.nullable),
        Some(false)
    );
    assert_eq!(
        c.column("shared_blks_read").and_then(|col| col.unit),
        Some(Unit::Count)
    );
    for name in ["first_call", "last_call"] {
        assert_eq!(
            c.column(name).and_then(|col| col.unit),
            Some(Unit::Microseconds)
        );
    }
    assert_eq!(c.column("plan").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("datname").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("usename").map(|col| col.nullable), Some(true));
    assert!(c.column("slow_log_calls").is_some());
    assert!(c.column("local_blks_hit").is_some());
    assert!(c.column("local_blks_dirtied").is_some());
    assert!(c.column("mean_plan_time").is_some());
    // The vadv fork sums I/O timings; the split ossc columns must not leak in.
    assert!(c.column("shared_blk_read_time").is_none());
}

#[test]
fn vadv_v1_roundtrip_preserves_null_plan() {
    crate::assert_roundtrips(&[
        row(1_000, 5, 10, 42, Some(StrId(77))),
        row(1_000, 5, 11, 43, None),
    ]);
}

#[test]
fn vadv_v1_encode_sorts_by_key() {
    let bytes = PgStorePlansVadvV1::encode(&[
        row(1_000, 9, 3, 4, None),
        row(1_000, 1, 8, 3, None),
        row(1_000, 1, 2, 9, None),
        row(1_000, 1, 2, 1, None),
    ])
    .expect("encode");
    let decoded =
        PgStorePlansVadvV1::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded
            .iter()
            .map(|r| (r.dbid, r.userid, r.queryid))
            .collect::<Vec<_>>(),
        [(1, 2, 1), (1, 2, 9), (1, 8, 3), (9, 3, 4)]
    );
}
