use super::{PgStatUserTablesV1, PgStatUserTablesV2, PgStatUserTablesV3, PgStatUserTablesV4};
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn assert_common_contract(c: crate::TypeContract) {
    assert_eq!(
        c.column("xid_age").map(|column| column.nullable),
        Some(true)
    );
    assert_eq!(
        c.column("mxid_age").map(|column| column.nullable),
        Some(true)
    );
    for name in [
        "last_vacuum",
        "last_autovacuum",
        "last_analyze",
        "last_autoanalyze",
        "toast_last_autovacuum",
    ] {
        assert_eq!(
            c.column(name).and_then(|column| column.unit),
            Some(Unit::Microseconds)
        );
    }
    for name in ["last_seq_scan", "last_idx_scan"] {
        if let Some(column) = c.column(name) {
            assert_eq!(column.unit, Some(Unit::Microseconds));
        }
    }
}

fn v4_row(ts: i64, datid: u32, relid: u32) -> PgStatUserTablesV4 {
    PgStatUserTablesV4 {
        ts: Ts(ts),
        datid,
        datname: StrId(u64::from(datid) | 1),
        relid,
        schemaname: StrId(2),
        relname: StrId(u64::from(relid) | 1),
        tablespace_oid: Some(1_663),
        tablespace: Some(StrId(4)),
        seq_scan: 10,
        seq_tup_read: 1_000,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: 50,
        n_tup_upd: 30,
        n_tup_del: 10,
        n_tup_hot_upd: 5,
        n_tup_newpage_upd: 0,
        n_live_tup: 900,
        n_dead_tup: 40,
        n_mod_since_analyze: 70,
        n_ins_since_vacuum: 20,
        vacuum_count: 1,
        autovacuum_count: 3,
        analyze_count: 1,
        autoanalyze_count: 2,
        last_vacuum: Some(Ts(ts - 10)),
        last_autovacuum: None,
        last_analyze: None,
        last_autoanalyze: Some(Ts(ts - 5)),
        last_seq_scan: Some(Ts(ts - 1)),
        last_idx_scan: None,
        total_vacuum_time: 12.5,
        total_autovacuum_time: 340.0,
        total_analyze_time: 7.5,
        total_autoanalyze_time: 21.0,
        main_fork_bytes: 8_192,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        xid_age: Some(100_000_000),
        mxid_age: Some(5_000_000),
        reltuples: 900,
        heap_blks_read: 400,
        heap_blks_hit: 90_000,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
    }
}

#[test]
fn v4_contract_shape() {
    let c = PgStatUserTablesV4::CONTRACT;
    assert_eq!(c.type_id.get(), 1_013_008);
    assert_eq!(c.columns.len(), 51);
    assert_eq!(c.sort_key, ["datid", "relid", "ts"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("relid").map(|col| col.nullable), Some(false));
    assert_eq!(
        c.column("tablespace_oid").map(|col| col.nullable),
        Some(true)
    );
    assert_eq!(c.column("tablespace").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(true));
    assert!(c.column("total_vacuum_time").is_some());
    assert!(c.column("total_autovacuum_time").is_some());
    assert!(c.column("total_analyze_time").is_some());
    assert!(c.column("total_autoanalyze_time").is_some());
    assert!(c.column("main_fork_bytes").is_some());
    assert!(c.column("size_bytes").is_none());
    assert_common_contract(c);
}

#[test]
fn v4_roundtrip() {
    let mut null_ages = v4_row(5, 5, 16_384);
    null_ages.xid_age = None;
    null_ages.mxid_age = None;
    crate::assert_roundtrips(&[
        null_ages,
        v4_row(1_000, 5, 16_384),
        v4_row(1_000, 5, 16_385),
    ]);
}

#[test]
fn v3_contract_shape() {
    let c = PgStatUserTablesV3::CONTRACT;
    assert_eq!(c.type_id.get(), 1_013_007);
    assert_eq!(c.columns.len(), 47);
    assert_eq!(c.sort_key, ["datid", "relid", "ts"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("relid").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("datname").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("toast_bytes").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("last_vacuum").map(|col| col.nullable), Some(true));
    assert!(c.column("n_tup_newpage_upd").is_some());
    assert!(c.column("last_seq_scan").is_some());
    assert_common_contract(c);
}

#[test]
fn v2_drops_pg16_columns() {
    let c = PgStatUserTablesV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_013_006);
    assert_eq!(c.columns.len(), 44);
    assert!(c.column("n_ins_since_vacuum").is_some());
    assert!(c.column("n_tup_newpage_upd").is_none());
    assert!(c.column("last_seq_scan").is_none());
    assert_common_contract(c);
}

#[test]
fn v1_is_base_layout() {
    let c = PgStatUserTablesV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_013_005);
    assert_eq!(c.columns.len(), 43);
    assert!(c.column("n_ins_since_vacuum").is_none());
    assert_common_contract(c);
}

fn v3_row(ts: i64, datid: u32, relid: u32) -> PgStatUserTablesV3 {
    PgStatUserTablesV3 {
        ts: Ts(ts),
        datid,
        datname: StrId(u64::from(datid) | 1),
        relid,
        schemaname: StrId(2),
        relname: StrId(u64::from(relid) | 1),
        tablespace_oid: Some(1_663),
        tablespace: Some(StrId(4)),
        seq_scan: 10,
        seq_tup_read: 1_000,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: 50,
        n_tup_upd: 30,
        n_tup_del: 10,
        n_tup_hot_upd: 5,
        n_tup_newpage_upd: 0,
        n_live_tup: 900,
        n_dead_tup: 40,
        n_mod_since_analyze: 70,
        n_ins_since_vacuum: 20,
        vacuum_count: 1,
        autovacuum_count: 3,
        analyze_count: 1,
        autoanalyze_count: 2,
        last_vacuum: Some(Ts(ts - 10)),
        last_autovacuum: None,
        last_analyze: None,
        last_autoanalyze: Some(Ts(ts - 5)),
        last_seq_scan: Some(Ts(ts - 1)),
        last_idx_scan: None,
        main_fork_bytes: 8_192,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        xid_age: Some(100_000_000),
        mxid_age: Some(5_000_000),
        reltuples: 900,
        heap_blks_read: 400,
        heap_blks_hit: 90_000,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
    }
}

#[test]
fn v3_roundtrip() {
    crate::assert_roundtrips(&[v3_row(1_000, 5, 16_384), v3_row(1_000, 5, 16_385)]);
}

#[test]
fn v3_encode_sorts_by_datid_relid_ts() {
    let bytes = PgStatUserTablesV3::encode(&[
        v3_row(1_000, 9, 16_385),
        v3_row(1_000, 1, 16_390),
        v3_row(1_000, 1, 16_384),
    ])
    .expect("encode");
    let decoded =
        PgStatUserTablesV3::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded
            .iter()
            .map(|r| (r.datid, r.relid))
            .collect::<Vec<_>>(),
        [(1, 16_384), (1, 16_390), (9, 16_385)]
    );
}
