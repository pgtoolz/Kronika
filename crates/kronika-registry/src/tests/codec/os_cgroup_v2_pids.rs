use super::OsCgroupV2Pids;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_and_missing_values_roundtrip() {
    let contract = OsCgroupV2Pids::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_209_001);
    assert_eq!(
        contract.identity,
        ["cgroup_path", "cgroup_identity", "events_source"],
    );
    assert_eq!(lint(&[contract]), Ok(()));
    assert_eq!(
        crate::contract(contract.type_id.get())
            .expect("registered type")
            .name,
        contract.name,
    );
    let rows = [
        OsCgroupV2Pids {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            current: Some(0),
            max: Some(41),
            max_unlimited: Some(false),
            failure_max: Some(42),
            events_source: 1,
        },
        OsCgroupV2Pids {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(3),
            current: None,
            max: None,
            max_unlimited: None,
            failure_max: None,
            events_source: 0,
        },
    ];
    crate::assert_roundtrips(&rows);
    crate::assert_roundtrips(&[
        rows[1],
        OsCgroupV2Pids {
            ts: Ts(3),
            max_unlimited: Some(true),
            ..rows[1]
        },
    ]);
}
