use super::{PgStatUserIndexesV1, PgStatUserIndexesV2};
use crate::{Section, StrId, Ts, Unit, VerifiedSection};

fn v2_row(ts: i64, datid: u32, indexrelid: u32) -> PgStatUserIndexesV2 {
    PgStatUserIndexesV2 {
        ts: Ts(ts),
        datid,
        datname: StrId(u64::from(datid) | 1),
        indexrelid,
        relid: indexrelid - 1,
        schemaname: StrId(2),
        relname: StrId(3),
        indexrelname: StrId(u64::from(indexrelid) | 1),
        tablespace_oid: 1_663,
        tablespace: Some(StrId(4)),
        idx_scan: 120,
        idx_tup_read: 3_400,
        idx_tup_fetch: 3_000,
        main_fork_bytes: 16_384,
        last_idx_scan: Some(Ts(ts - 1)),
        indisunique: true,
        indisprimary: true,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname: StrId(5),
        indexdef: Some(StrId(6)),
        idx_blks_read: 40,
        idx_blks_hit: 9_000,
    }
}

#[test]
fn v2_contract_shape() {
    let c = PgStatUserIndexesV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_014_004);
    assert_eq!(c.columns.len(), 24);
    assert_eq!(c.sort_key, ["datid", "indexrelid", "ts"]);
    assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
    assert_eq!(c.column("indexrelid").map(|col| col.nullable), Some(false));
    assert_eq!(
        c.column("tablespace_oid").map(|col| col.nullable),
        Some(false)
    );
    assert_eq!(c.column("tablespace").map(|col| col.nullable), Some(true));
    assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(false));
    assert_eq!(
        c.column("last_idx_scan").map(|col| col.nullable),
        Some(true)
    );
    assert!(c.column("main_fork_bytes").is_some());
    assert!(c.column("size_bytes").is_none());
    assert!(c.column("indisunique").is_some());
    assert_eq!(
        c.column("indisexclusion").map(|col| col.nullable),
        Some(false)
    );
    assert_eq!(c.column("indisready").map(|col| col.nullable), Some(false));
    assert!(c.column("amname").is_some());
    assert_eq!(c.column("indexdef").map(|col| col.nullable), Some(true));
    assert_eq!(
        c.column("last_idx_scan").and_then(|col| col.unit),
        Some(Unit::Microseconds)
    );
}

#[test]
fn v1_is_base_layout() {
    let c = PgStatUserIndexesV1::CONTRACT;
    assert_eq!(c.type_id.get(), 1_014_003);
    assert_eq!(c.columns.len(), 23);
    assert_eq!(c.sort_key, ["datid", "indexrelid", "ts"]);
    assert!(c.column("last_idx_scan").is_none());
    assert!(c.column("main_fork_bytes").is_some());
    assert!(c.column("idx_blks_hit").is_some());
    assert!(c.column("indisexclusion").is_some());
    assert!(c.column("indisready").is_some());
    assert_eq!(c.column("indexdef").map(|col| col.nullable), Some(true));
}

#[test]
fn v2_roundtrip() {
    let mut never_scanned = v2_row(5, 5, 16_384);
    never_scanned.last_idx_scan = None;
    never_scanned.indexdef = None;
    crate::assert_roundtrips(&[
        never_scanned,
        v2_row(1_000, 5, 16_384),
        v2_row(1_000, 5, 16_385),
    ]);
}

#[test]
fn v2_encode_sorts_by_datid_indexrelid_ts() {
    let bytes = PgStatUserIndexesV2::encode(&[
        v2_row(1_000, 9, 16_385),
        v2_row(1_000, 1, 16_390),
        v2_row(1_000, 1, 16_384),
    ])
    .expect("encode");
    let decoded =
        PgStatUserIndexesV2::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded
            .iter()
            .map(|r| (r.datid, r.indexrelid))
            .collect::<Vec<_>>(),
        [(1, 16_384), (1, 16_390), (9, 16_385)]
    );
}
