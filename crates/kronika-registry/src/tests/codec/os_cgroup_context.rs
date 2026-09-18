use super::OsCgroupContext;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_shape_and_nulls_roundtrip() {
    let contract = OsCgroupContext::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_205_001);
    assert_eq!(contract.sort_key, ["ts"]);
    assert!(contract.identity.is_empty());
    assert_eq!(
        contract
            .columns
            .iter()
            .map(|column| column.name)
            .collect::<Vec<_>>(),
        [
            "ts",
            "cgroup_version",
            "cpu_path",
            "memory_path",
            "io_path",
            "cpuset_cpus",
            "effective_cpu_quota_usec",
            "effective_cpu_period_usec",
            "effective_memory_max",
            "scope",
        ]
    );
    assert_eq!(lint(&[contract]), Ok(()));

    crate::assert_roundtrips(&[
        OsCgroupContext {
            ts: Ts(1),
            cgroup_version: 2,
            cpu_path: Some(StrId(10)),
            memory_path: Some(StrId(10)),
            io_path: Some(StrId(10)),
            cpuset_cpus: Some(4),
            effective_cpu_quota_usec: Some(150_000),
            effective_cpu_period_usec: Some(100_000),
            effective_memory_max: Some(536_870_912),
            scope: 3,
        },
        OsCgroupContext {
            ts: Ts(2),
            cgroup_version: 0,
            cpu_path: None,
            memory_path: None,
            io_path: None,
            cpuset_cpus: None,
            effective_cpu_quota_usec: None,
            effective_cpu_period_usec: None,
            effective_memory_max: None,
            scope: 4,
        },
    ]);
}
