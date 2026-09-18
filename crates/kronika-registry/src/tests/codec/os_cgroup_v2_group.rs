use super::OsCgroupV2Group;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_and_missing_values_roundtrip() {
    let contract = OsCgroupV2Group::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_206_001);
    assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
    assert_eq!(lint(&[contract]), Ok(()));
    assert_eq!(
        crate::contract(contract.type_id.get())
            .expect("registered type")
            .name,
        contract.name,
    );
    crate::assert_roundtrips(&[
        OsCgroupV2Group {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            mount_root: StrId(4),
            parent_identity: Some(StrId(5)),
            memory_localevents: true,
            pids_localevents: true,
        },
        OsCgroupV2Group {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(3),
            mount_root: StrId(4),
            parent_identity: None,
            memory_localevents: false,
            pids_localevents: false,
        },
    ]);
}
