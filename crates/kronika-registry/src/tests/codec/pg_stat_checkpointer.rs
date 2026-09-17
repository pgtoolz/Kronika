use super::{PgStatCheckpointerV1, PgStatCheckpointerV2};
use crate::{Section, Ts};

#[test]
fn contracts_match_postgresql_versions() {
    let v1 = PgStatCheckpointerV1::CONTRACT;
    let v2 = PgStatCheckpointerV2::CONTRACT;
    assert_eq!(v1.type_id.get(), 1_017_001);
    assert_eq!(v1.columns.len(), 10);
    assert!(v1.column("num_done").is_none());
    assert!(v1.column("slru_written").is_none());
    assert_eq!(v2.type_id.get(), 1_017_002);
    assert_eq!(v2.columns.len(), 12);
    assert!(v2.column("num_done").is_some());
    assert!(v2.column("slru_written").is_some());
}

#[test]
fn layouts_roundtrip() {
    crate::assert_roundtrips(&[PgStatCheckpointerV1 {
        ts: Ts(2_000_000),
        num_timed: 10,
        num_requested: 3,
        restartpoints_timed: 2,
        restartpoints_req: 1,
        restartpoints_done: 2,
        write_time: 45.5,
        sync_time: 3.25,
        buffers_written: 1000,
        stats_reset: Some(Ts(1_000_000)),
    }]);
    crate::assert_roundtrips(&[PgStatCheckpointerV2 {
        ts: Ts(3_000_000),
        num_timed: 10,
        num_requested: 3,
        num_done: 11,
        restartpoints_timed: 2,
        restartpoints_req: 1,
        restartpoints_done: 2,
        write_time: 45.5,
        sync_time: 3.25,
        buffers_written: 1000,
        slru_written: 20,
        stats_reset: None,
    }]);
}
