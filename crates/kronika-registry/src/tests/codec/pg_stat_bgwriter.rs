use super::{PgStatBgwriterV1, PgStatBgwriterV2};
use crate::{Section, Ts};

#[test]
fn contracts_match_postgresql_versions() {
    let v1 = PgStatBgwriterV1::CONTRACT;
    let v2 = PgStatBgwriterV2::CONTRACT;
    assert_eq!(v1.type_id.get(), 1_006_001);
    assert_eq!(v1.columns.len(), 12);
    assert!(v1.column("checkpoints_timed").is_some());
    assert_eq!(v2.type_id.get(), 1_006_002);
    assert_eq!(v2.columns.len(), 5);
    assert!(v2.column("checkpoints_timed").is_none());
}

#[test]
fn layouts_roundtrip() {
    crate::assert_roundtrips(&[PgStatBgwriterV1 {
        ts: Ts(2_000_000),
        checkpoints_timed: 4,
        checkpoints_req: 2,
        checkpoint_write_time: 3.5,
        checkpoint_sync_time: 1.25,
        buffers_checkpoint: 100,
        buffers_clean: 200,
        maxwritten_clean: 3,
        buffers_backend: 40,
        buffers_backend_fsync: 1,
        buffers_alloc: 500,
        stats_reset: Some(Ts(1_000_000)),
    }]);
    crate::assert_roundtrips(&[PgStatBgwriterV2 {
        ts: Ts(3_000_000),
        buffers_clean: 200,
        maxwritten_clean: 3,
        buffers_alloc: 500,
        stats_reset: None,
    }]);
}
