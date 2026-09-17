use super::OsCgroupCpuV3;
use crate::{Section, StrId, Ts};

#[test]
fn ancestor_cpu_nulls_roundtrip() {
    assert_eq!(OsCgroupCpuV3::CONTRACT.type_id.get(), 1_201_003);
    assert_eq!(
        OsCgroupCpuV3::CONTRACT.identity,
        ["cgroup_path", "cgroup_identity"]
    );
    crate::assert_roundtrips(&[
        OsCgroupCpuV3 {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            usage_usec: 100,
            user_usec: 60,
            system_usec: 40,
            throttled_usec: None,
            nr_throttled: None,
            quota_usec: None,
            period_usec: None,
            scope: 4,
        },
        OsCgroupCpuV3 {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            usage_usec: 150,
            user_usec: 90,
            system_usec: 60,
            throttled_usec: Some(0),
            nr_throttled: Some(0),
            quota_usec: Some(150_000),
            period_usec: Some(100_000),
            scope: 4,
        },
    ]);
}
