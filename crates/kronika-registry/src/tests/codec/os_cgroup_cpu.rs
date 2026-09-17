use super::{OsCgroupCpu, OsCgroupCpuV2};
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(
        lint(&[OsCgroupCpu::CONTRACT, OsCgroupCpuV2::CONTRACT]),
        Ok(())
    );
}

#[test]
fn contract_shape() {
    let c = OsCgroupCpu::CONTRACT;
    assert_eq!(c.type_id.get(), 1_201_001);
    assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
    assert_eq!(c.identity, ["cgroup_path"]);
}

#[test]
fn v2_contract_shape() {
    let c = OsCgroupCpuV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_201_002);
    assert_eq!(
        c.columns
            .iter()
            .map(|column| column.name)
            .collect::<Vec<_>>(),
        [
            "ts",
            "cgroup_path",
            "usage_usec",
            "user_usec",
            "system_usec",
            "throttled_usec",
            "nr_throttled",
            "quota_usec",
            "period_usec",
            "cpuset_cpus",
            "scope",
        ]
    );
    assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
    assert_eq!(c.identity, ["cgroup_path"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[OsCgroupCpu {
        ts: Ts(1),
        cgroup_path: StrId(10),
        usage_usec: 100,
        user_usec: 60,
        system_usec: 40,
        throttled_usec: 7,
        nr_throttled: 2,
        quota_usec: -1,
        period_usec: 100_000,
        scope: 1,
    }]);
}

#[test]
fn v2_roundtrip() {
    crate::assert_roundtrips(&[OsCgroupCpuV2 {
        ts: Ts(1),
        cgroup_path: StrId(10),
        usage_usec: 100,
        user_usec: 60,
        system_usec: 40,
        throttled_usec: 7,
        nr_throttled: 2,
        quota_usec: -1,
        period_usec: 100_000,
        cpuset_cpus: Some(4),
        scope: 1,
    }]);
}
