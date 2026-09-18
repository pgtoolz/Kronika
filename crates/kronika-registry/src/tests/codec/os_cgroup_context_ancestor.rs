use super::OsCgroupContextV2;
use crate::{Section, StrId, Ts};

#[test]
fn selected_context_identity_roundtrip() {
    assert_eq!(OsCgroupContextV2::CONTRACT.type_id.get(), 1_205_002);
    let row = OsCgroupContextV2 {
        ts: Ts(1),
        cgroup_version: 1,
        cpu_path: None,
        memory_path: Some(StrId(1)),
        io_path: None,
        pids_path: Some(StrId(2)),
        cpu_identity: None,
        memory_identity: Some(StrId(3)),
        io_identity: None,
        pids_identity: Some(StrId(4)),
        cpu_root: None,
        memory_root: Some(StrId(5)),
        io_root: None,
        pids_root: Some(StrId(6)),
        cpuset_cpus: None,
        effective_cpu_quota_usec: None,
        effective_cpu_period_usec: None,
        effective_memory_max: Some(8192),
        scope: 4,
    };
    crate::assert_roundtrips(&[
        row,
        OsCgroupContextV2 {
            ts: Ts(2),
            memory_identity: Some(StrId(7)),
            ..row
        },
    ]);
}
