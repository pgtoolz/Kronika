use super::PgStorePlansOsscV1;
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn row(ts: i64, dbid: u32, queryid: i64, plan: Option<StrId>) -> PgStorePlansOsscV1 {
    PgStorePlansOsscV1 {
        ts: Ts(ts),
        queryid,
        planid: -7,
        userid: 10,
        dbid,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        plan,
        calls: 4,
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
        first_call: Ts(ts - 1_000),
        last_call: Ts(ts - 1),
    }
}

#[test]
fn ossc_v1_contract_shape() {
    let c = PgStorePlansOsscV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_003_001);
    assert_eq!(c.columns.len(), 33);
    assert_eq!(c.identity, ["userid", "dbid", "queryid", "planid"]);
    assert_eq!(c.sort_key, ["dbid", "userid", "queryid", "planid"]);
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
    // Upstream keys entries with the real core query id.
    assert_eq!(c.column("queryid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("plan").map(|col| col.nullable), Some(true));
    assert!(c.column("shared_blk_read_time").is_some());
    assert!(c.column("temp_blk_write_time").is_some());
    // vadv-only columns must not leak into the upstream layout.
    assert!(c.column("queryid_stat_statements").is_none());
    assert!(c.column("slow_log_calls").is_none());
    assert!(c.column("total_plan_time").is_none());
}

#[test]
fn ossc_v1_roundtrip_preserves_null_plan() {
    crate::assert_roundtrips(&[row(1_000, 5, 42, Some(StrId(77))), row(1_000, 5, 43, None)]);
}

#[test]
fn ossc_v1_encode_sorts_by_key() {
    let bytes = PgStorePlansOsscV1::encode(&[
        row(1_000, 9, 3, None),
        row(1_000, 1, 8, None),
        row(1_000, 1, 2, None),
    ])
    .expect("encode");
    let decoded =
        PgStorePlansOsscV1::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded
            .iter()
            .map(|r| (r.dbid, r.queryid))
            .collect::<Vec<_>>(),
        [(1, 2), (1, 8), (9, 3)]
    );
}
