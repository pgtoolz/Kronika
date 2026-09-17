use super::OsCgroupMemoryV3;
use crate::{Section, StrId, Ts};

#[test]
fn ancestor_memory_nulls_roundtrip() {
    assert_eq!(OsCgroupMemoryV3::CONTRACT.type_id.get(), 1_202_003);
    assert_eq!(
        OsCgroupMemoryV3::CONTRACT.identity,
        ["cgroup_path", "cgroup_identity"]
    );
    let row = OsCgroupMemoryV3 {
        ts: Ts(1),
        cgroup_path: StrId(1),
        cgroup_identity: StrId(2),
        current: 4096,
        max: None,
        max_unlimited: None,
        anon: Some(100),
        file: Some(200),
        kernel: None,
        slab: None,
        low_events: None,
        high_events: None,
        max_events: None,
        oom_events: None,
        oom_kill: None,
        scope: 4,
    };
    crate::assert_roundtrips(&[
        row,
        OsCgroupMemoryV3 {
            ts: Ts(2),
            max_unlimited: Some(true),
            ..row
        },
        OsCgroupMemoryV3 {
            ts: Ts(3),
            max: Some(8192),
            max_unlimited: Some(false),
            ..row
        },
    ]);
}
