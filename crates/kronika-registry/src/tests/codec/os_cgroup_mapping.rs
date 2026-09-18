use super::OsCgroupMapping;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsCgroupMapping::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsCgroupMapping::CONTRACT;
    assert_eq!(c.type_id.get(), 1_200_001);
    assert_eq!(c.sort_key, ["pid", "ts"]);
    assert_eq!(c.identity, ["pid"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[OsCgroupMapping {
        ts: Ts(1),
        pid: 10,
        starttime: Ts(100),
        cgroup_path: StrId(11),
        scope: 1,
    }]);
}
