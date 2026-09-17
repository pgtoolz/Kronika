use super::OsCgroupV2Io;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_and_missing_values_roundtrip() {
    let contract = OsCgroupV2Io::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_210_001);
    assert_eq!(
        contract.identity,
        ["cgroup_path", "cgroup_identity", "major", "minor"]
    );
    assert_eq!(lint(&[contract]), Ok(()));
    assert_eq!(
        crate::contract(contract.type_id.get())
            .expect("registered type")
            .name,
        contract.name,
    );
    crate::assert_roundtrips(&[
        OsCgroupV2Io {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            major: 8,
            minor: 0,
            rbytes: Some(0),
            wbytes: Some(41),
            rios: Some(42),
            wios: Some(43),
        },
        OsCgroupV2Io {
            ts: Ts(2),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(3),
            major: 8,
            minor: 0,
            rbytes: None,
            wbytes: None,
            rios: None,
            wios: None,
        },
    ]);
}
