use super::OsCgroupPids;
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn row(max: Option<i64>) -> OsCgroupPids {
    OsCgroupPids {
        ts: Ts(1),
        cgroup_path: StrId(10),
        current: 12,
        max,
        scope: 1,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsCgroupPids::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsCgroupPids::CONTRACT;
    assert_eq!(c.type_id.get(), 1_204_001);
    assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
    assert_eq!(c.identity, ["cgroup_path"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[row(Some(100)), row(None)]);
}

#[test]
fn unlimited_limit_survives_as_null() {
    let bytes = OsCgroupPids::encode(&[row(None)]).expect("encode");
    let decoded = OsCgroupPids::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].max, None);
}
