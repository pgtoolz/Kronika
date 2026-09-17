use super::OsCgroupIoV2;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn ancestor_io_identity_and_unknown_counters_roundtrip() {
    let contract = OsCgroupIoV2::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_203_003);
    assert_eq!(
        contract.identity,
        ["cgroup_path", "cgroup_identity", "major", "minor"]
    );
    assert_eq!(lint(&[contract]), Ok(()));
    let row = OsCgroupIoV2 {
        ts: Ts(1),
        cgroup_path: StrId(1),
        cgroup_identity: StrId(2),
        major: 8,
        minor: 0,
        rbytes: Some(100),
        wbytes: None,
        rios: None,
        wios: None,
        scope: 4,
    };
    crate::assert_roundtrips(&[
        row,
        OsCgroupIoV2 {
            ts: Ts(2),
            cgroup_identity: StrId(3),
            rbytes: Some(900),
            ..row
        },
    ]);
}
