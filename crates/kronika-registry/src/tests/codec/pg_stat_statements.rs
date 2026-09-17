use super::{
    PgStatStatementsV1, PgStatStatementsV2, PgStatStatementsV3, PgStatStatementsV4,
    PgStatStatementsV5, PgStatStatementsV6,
};
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn assert_shared_block_unit(c: crate::TypeContract) {
    assert_eq!(
        c.column("shared_blks_read").and_then(|column| column.unit),
        Some(Unit::Count)
    );
}

fn assert_stats_timestamp_units(c: crate::TypeContract) {
    for name in ["stats_since", "minmax_stats_since"] {
        let column = c.column(name).expect("statistics timestamp");
        assert_eq!(column.unit, Some(Unit::Microseconds));
        assert!(!column.nullable);
    }
}

fn v6_row(ts: i64, dbid: u32, userid: u32, queryid: Option<i64>) -> PgStatStatementsV6 {
    PgStatStatementsV6 {
        ts: Ts(ts),
        queryid,
        userid,
        dbid,
        toplevel: true,
        datname: Some(StrId(u64::from(dbid) | 1)),
        usename: Some(StrId(u64::from(userid) | 1)),
        query: queryid.map(|q| StrId(q.cast_unsigned() | 1)),
        calls: 100,
        rows: 5_000,
        plans: 90,
        total_exec_time: 1_234.5,
        total_plan_time: 12.5,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        shared_blk_read_time: 12.5,
        shared_blk_write_time: 3.0,
        local_blk_read_time: 0.0,
        local_blk_write_time: 0.0,
        temp_blk_read_time: 0.0,
        temp_blk_write_time: 0.0,
        wal_records: 42,
        wal_fpi: 3,
        wal_bytes: 8_192,
        wal_buffers_full: 1,
        jit_functions: 0,
        jit_generation_time: 0.0,
        jit_inlining_count: 0,
        jit_inlining_time: 0.0,
        jit_optimization_count: 0,
        jit_optimization_time: 0.0,
        jit_emission_count: 0,
        jit_emission_time: 0.0,
        jit_deform_count: 0,
        jit_deform_time: 0.0,
        parallel_workers_to_launch: 4,
        parallel_workers_launched: 3,
        stats_since: Ts(ts - 100),
        minmax_stats_since: Ts(ts - 50),
    }
}

#[test]
fn v6_contract_shape() {
    let c = PgStatStatementsV6::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_006);
    assert_eq!(c.columns.len(), 55);
    assert_eq!(c.sort_key, ["dbid", "userid", "queryid", "toplevel", "ts"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("dbid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("userid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("queryid").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("query").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("datname").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("toplevel").map(|col| col.nullable), Some(false));
    assert!(c.column("wal_buffers_full").is_some());
    assert!(c.column("parallel_workers_launched").is_some());
    assert!(c.column("shared_blk_read_time").is_some());
    assert!(c.column("local_blk_read_time").is_some());
    assert!(c.column("jit_deform_time").is_some());
    assert_eq!(c.column("stats_since").map(|col| col.nullable), Some(false));
    assert_shared_block_unit(c);
    assert_stats_timestamp_units(c);
    // No legacy names on the newest layout.
    assert!(c.column("total_time").is_none());
    assert!(c.column("blk_read_time").is_none());
}

#[test]
fn v6_roundtrip_and_null_preservation() {
    // A row with a resolved queryid/query and one with nullable identity
    // and text, as produced under restricted visibility.
    crate::assert_roundtrips(&[v6_row(1_000, 5, 10, Some(777)), v6_row(1_000, 5, 11, None)]);
}

#[test]
fn v6_encode_sorts_by_dbid_then_userid_then_ts() {
    let bytes = PgStatStatementsV6::encode(&[
        v6_row(1_000, 9, 3, Some(1)),
        v6_row(1_000, 1, 8, Some(2)),
        v6_row(1_000, 1, 2, Some(3)),
    ])
    .expect("encode");
    let decoded =
        PgStatStatementsV6::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded
            .iter()
            .map(|r| (r.dbid, r.userid))
            .collect::<Vec<_>>(),
        [(1, 2), (1, 8), (9, 3)]
    );
}

fn v5_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV5 {
    PgStatStatementsV5 {
        ts: Ts(ts),
        queryid: Some(777),
        userid,
        dbid,
        toplevel: true,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        query: Some(StrId(3)),
        calls: 100,
        rows: 5_000,
        plans: 90,
        total_exec_time: 1_234.5,
        total_plan_time: 12.5,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        shared_blk_read_time: 12.5,
        shared_blk_write_time: 3.0,
        local_blk_read_time: 0.0,
        local_blk_write_time: 0.0,
        temp_blk_read_time: 0.0,
        temp_blk_write_time: 0.0,
        wal_records: 42,
        wal_fpi: 3,
        wal_bytes: 8_192,
        jit_functions: 0,
        jit_generation_time: 0.0,
        jit_inlining_count: 0,
        jit_inlining_time: 0.0,
        jit_optimization_count: 0,
        jit_optimization_time: 0.0,
        jit_emission_count: 0,
        jit_emission_time: 0.0,
        jit_deform_count: 0,
        jit_deform_time: 0.0,
        stats_since: Ts(ts - 100),
        minmax_stats_since: Ts(ts - 50),
    }
}

#[test]
fn v5_contract_shape() {
    let c = PgStatStatementsV5::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_005);
    assert_eq!(c.columns.len(), 52);
    assert!(c.column("shared_blk_read_time").is_some());
    assert!(c.column("local_blk_write_time").is_some());
    assert!(c.column("jit_deform_count").is_some());
    assert_eq!(c.column("stats_since").map(|col| col.nullable), Some(false));
    // 1.11 renamed away the unqualified block-timing names and has no 1.12
    // columns.
    assert!(c.column("blk_read_time").is_none());
    assert!(c.column("wal_buffers_full").is_none());
    assert!(c.column("parallel_workers_launched").is_none());
    assert_shared_block_unit(c);
    assert_stats_timestamp_units(c);
}

#[test]
fn v5_roundtrip() {
    crate::assert_roundtrips(&[v5_row(1_000, 5, 10), v5_row(1_000, 5, 11)]);
}

fn v4_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV4 {
    PgStatStatementsV4 {
        ts: Ts(ts),
        queryid: Some(777),
        userid,
        dbid,
        toplevel: true,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        query: Some(StrId(3)),
        calls: 100,
        rows: 5_000,
        plans: 90,
        total_exec_time: 1_234.5,
        total_plan_time: 12.5,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        temp_blk_read_time: 0.0,
        temp_blk_write_time: 0.0,
        wal_records: 42,
        wal_fpi: 3,
        wal_bytes: 8_192,
        jit_functions: 0,
        jit_generation_time: 0.0,
        jit_inlining_count: 0,
        jit_inlining_time: 0.0,
        jit_optimization_count: 0,
        jit_optimization_time: 0.0,
        jit_emission_count: 0,
        jit_emission_time: 0.0,
    }
}

#[test]
fn v4_contract_shape() {
    let c = PgStatStatementsV4::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_004);
    assert_eq!(c.columns.len(), 46);
    // 1.10 uses the unqualified block-timing names and has temp timing + JIT.
    assert!(c.column("blk_read_time").is_some());
    assert!(c.column("temp_blk_read_time").is_some());
    assert!(c.column("jit_emission_time").is_some());
    // No 1.11 columns.
    assert!(c.column("shared_blk_read_time").is_none());
    assert!(c.column("local_blk_read_time").is_none());
    assert!(c.column("jit_deform_count").is_none());
    assert!(c.column("stats_since").is_none());
    assert_shared_block_unit(c);
}

#[test]
fn v4_roundtrip() {
    crate::assert_roundtrips(&[v4_row(1_000, 5, 10), v4_row(1_000, 5, 11)]);
}

fn v3_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV3 {
    PgStatStatementsV3 {
        ts: Ts(ts),
        queryid: Some(777),
        userid,
        dbid,
        toplevel: true,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        query: Some(StrId(3)),
        calls: 100,
        rows: 5_000,
        plans: 90,
        total_exec_time: 1_234.5,
        total_plan_time: 12.5,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        wal_records: 42,
        wal_fpi: 3,
        wal_bytes: 8_192,
    }
}

#[test]
fn v3_contract_shape() {
    let c = PgStatStatementsV3::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_003);
    assert_eq!(c.columns.len(), 36);
    // 1.9 adds toplevel but not temp timing or JIT.
    assert!(c.column("toplevel").is_some());
    assert!(c.column("wal_bytes").is_some());
    assert!(c.column("temp_blk_read_time").is_none());
    assert!(c.column("jit_functions").is_none());
    assert_shared_block_unit(c);
}

#[test]
fn v3_roundtrip() {
    crate::assert_roundtrips(&[v3_row(1_000, 5, 10), v3_row(1_000, 5, 11)]);
}

fn v2_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(ts),
        queryid: Some(777),
        userid,
        dbid,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        query: Some(StrId(3)),
        calls: 100,
        rows: 5_000,
        plans: 90,
        total_exec_time: 1_234.5,
        total_plan_time: 12.5,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        wal_records: 42,
        wal_fpi: 3,
        wal_bytes: 8_192,
    }
}

#[test]
fn v2_contract_shape() {
    let c = PgStatStatementsV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_002);
    assert_eq!(c.columns.len(), 35);
    // 1.8 has the exec/plan split and WAL, but no toplevel.
    assert!(c.column("total_exec_time").is_some());
    assert!(c.column("total_plan_time").is_some());
    assert!(c.column("wal_records").is_some());
    assert!(c.column("toplevel").is_none());
    assert!(c.column("temp_blk_read_time").is_none());
    assert_shared_block_unit(c);
}

#[test]
fn v2_roundtrip() {
    crate::assert_roundtrips(&[v2_row(1_000, 5, 10), v2_row(1_000, 5, 11)]);
}

fn v1_row(ts: i64, dbid: u32, userid: u32) -> PgStatStatementsV1 {
    PgStatStatementsV1 {
        ts: Ts(ts),
        queryid: Some(777),
        userid,
        dbid,
        datname: Some(StrId(1)),
        usename: Some(StrId(2)),
        query: Some(StrId(3)),
        calls: 100,
        rows: 5_000,
        total_time: 1_234.5,
        min_time: 0.5,
        max_time: 40.0,
        mean_time: 12.3,
        stddev_time: 3.1,
        shared_blks_hit: 90_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 50,
        shared_blks_written: 30,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
    }
}

#[test]
fn v1_contract_shape() {
    let c = PgStatStatementsV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_002_001);
    assert_eq!(c.columns.len(), 26);
    // The legacy layout keeps the unqualified timing names and has no
    // exec/plan split, no WAL, no toplevel.
    assert!(c.column("total_time").is_some());
    assert!(c.column("mean_time").is_some());
    assert!(c.column("total_exec_time").is_none());
    assert!(c.column("total_plan_time").is_none());
    assert!(c.column("wal_records").is_none());
    assert!(c.column("toplevel").is_none());
    assert_shared_block_unit(c);
}

#[test]
fn v1_roundtrip_and_null_query() {
    let mut null_query = v1_row(5, 5, 10);
    null_query.query = None;
    null_query.queryid = None;
    crate::assert_roundtrips(&[null_query, v1_row(1_000, 5, 10), v1_row(1_000, 5, 11)]);
}
