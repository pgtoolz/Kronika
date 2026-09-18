use super::OsCgroupV2Memory;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_and_missing_values_roundtrip() {
    let contract = OsCgroupV2Memory::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_208_001);
    assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
    assert_eq!(lint(&[contract]), Ok(()));
    assert_eq!(
        crate::contract(contract.type_id.get())
            .expect("registered type")
            .name,
        contract.name,
    );
    let rows = [
        OsCgroupV2Memory {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            current: Some(0),
            max: Some(41),
            max_unlimited: Some(false),
            high: Some(42),
            high_unlimited: Some(false),
            anon: Some(43),
            file: Some(44),
            kernel: Some(45),
            slab: Some(46),
            low_events: Some(47),
            high_events: Some(48),
            max_events: Some(49),
            oom_events: Some(50),
            oom_kill: Some(51),
            local_high_events: Some(52),
            local_max_events: Some(53),
            local_oom_events: Some(54),
            local_oom_kill: Some(55),
            local_oom_group_kill: Some(56),
        },
        OsCgroupV2Memory {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(3),
            current: None,
            max: None,
            max_unlimited: None,
            high: None,
            high_unlimited: None,
            anon: None,
            file: None,
            kernel: None,
            slab: None,
            low_events: None,
            high_events: None,
            max_events: None,
            oom_events: None,
            oom_kill: None,
            local_high_events: None,
            local_max_events: None,
            local_oom_events: None,
            local_oom_kill: None,
            local_oom_group_kill: None,
        },
    ];
    crate::assert_roundtrips(&rows);
    crate::assert_roundtrips(&[
        rows[1],
        OsCgroupV2Memory {
            ts: Ts(3),
            max_unlimited: Some(true),
            high_unlimited: Some(true),
            ..rows[1]
        },
    ]);
}
