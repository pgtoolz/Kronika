use super::OsCgroupIo;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsCgroupIo::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsCgroupIo::CONTRACT;
    assert_eq!(c.type_id.get(), 1_203_002);
    assert_eq!(c.sort_key, ["cgroup_path", "major", "minor", "ts"]);
    assert_eq!(c.identity, ["cgroup_path", "major", "minor"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[OsCgroupIo {
        ts: Ts(1),
        cgroup_path: StrId(10),
        major: 8,
        minor: 0,
        rbytes: Some(100),
        wbytes: Some(200),
        rios: Some(3),
        wios: Some(4),
        scope: 1,
    }]);
}
