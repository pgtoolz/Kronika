use super::{PgStatWalV1, PgStatWalV2};
use crate::{Section, Ts};

fn v1_row(ts: i64) -> PgStatWalV1 {
    PgStatWalV1 {
        ts: Ts(ts),
        wal_records: 1_000_000,
        wal_fpi: 12_000,
        wal_bytes: 8_500_000_000,
        wal_buffers_full: 320,
        wal_write: 45_000,
        wal_sync: 44_000,
        wal_write_time: 1234.5,
        wal_sync_time: 678.0,
        stats_reset: Some(Ts(ts - 100_000)),
    }
}

fn v2_row(ts: i64) -> PgStatWalV2 {
    PgStatWalV2 {
        ts: Ts(ts),
        wal_records: 2_000_000,
        wal_fpi: 24_000,
        wal_bytes: 17_000_000_000,
        wal_buffers_full: 640,
        stats_reset: None,
    }
}

#[test]
fn contract_shape_matches_the_source() {
    let v1 = PgStatWalV1::CONTRACT;
    assert_eq!(v1.type_id.get(), 1_007_001);
    assert_eq!(v1.columns.len(), 10);
    assert_eq!(v1.sort_key, ["ts"]);
    assert_eq!(
        v1.column("wal_records").map(|col| col.nullable),
        Some(false)
    );
    assert_eq!(v1.column("stats_reset").map(|col| col.nullable), Some(true));

    let v2 = PgStatWalV2::CONTRACT;
    assert_eq!(v2.type_id.get(), 1_007_002);
    assert_eq!(v2.columns.len(), 6);
    assert_eq!(v2.column("wal_write"), None);
    assert_eq!(v2.column("stats_reset").map(|col| col.nullable), Some(true));
}

#[test]
fn roundtrip_preserves_values_and_nulls() {
    crate::assert_roundtrips(&[v1_row(1_000_000), v1_row(2_000_000)]);
    crate::assert_roundtrips(&[v2_row(3_000_000)]);
}
