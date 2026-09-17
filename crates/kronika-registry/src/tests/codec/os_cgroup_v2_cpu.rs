use super::OsCgroupV2Cpu;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_and_missing_values_roundtrip() {
    let contract = OsCgroupV2Cpu::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_207_001);
    assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
    assert_eq!(lint(&[contract]), Ok(()));
    assert_eq!(
        crate::contract(contract.type_id.get())
            .expect("registered type")
            .name,
        contract.name,
    );
    crate::assert_roundtrips(&[
        OsCgroupV2Cpu {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            usage_usec: Some(41),
            user_usec: Some(42),
            system_usec: Some(43),
            nr_periods: Some(44),
            nr_throttled: Some(45),
            throttled_usec: Some(46),
            quota_usec: Some(-1),
            period_usec: Some(100_000),
            cpuset_cpus: Some(8),
        },
        OsCgroupV2Cpu {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(3),
            usage_usec: None,
            user_usec: None,
            system_usec: None,
            nr_periods: None,
            nr_throttled: None,
            throttled_usec: None,
            quota_usec: None,
            period_usec: None,
            cpuset_cpus: None,
        },
    ]);
}
