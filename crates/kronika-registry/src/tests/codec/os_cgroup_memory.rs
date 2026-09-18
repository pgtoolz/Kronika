use super::{OsCgroupMemory, OsCgroupMemoryV2};
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn row(max: Option<i64>) -> OsCgroupMemory {
    OsCgroupMemory {
        ts: Ts(1),
        cgroup_path: StrId(10),
        current: 1024,
        max,
        anon: 100,
        file: 200,
        kernel: 30,
        slab: 20,
        low_events: 1,
        high_events: 2,
        max_events: 3,
        oom_events: 4,
        oom_kill: 5,
        scope: 1,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(
        lint(&[OsCgroupMemory::CONTRACT, OsCgroupMemoryV2::CONTRACT,]),
        Ok(())
    );
}

#[test]
fn contract_shape() {
    let c = OsCgroupMemory::CONTRACT;
    assert_eq!(c.type_id.get(), 1_202_001);
    assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
    assert_eq!(c.identity, ["cgroup_path"]);
}

#[test]
fn v2_contract_shape() {
    let c = OsCgroupMemoryV2::CONTRACT;
    assert_eq!(c.type_id.get(), 1_202_002);
    assert_eq!(
        c.columns
            .iter()
            .map(|column| column.name)
            .collect::<Vec<_>>(),
        [
            "ts",
            "cgroup_path",
            "current",
            "max",
            "anon",
            "file",
            "kernel",
            "slab",
            "shmem",
            "low_events",
            "high_events",
            "max_events",
            "oom_events",
            "oom_kill",
            "scope",
        ]
    );
    assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
    assert_eq!(c.identity, ["cgroup_path"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[row(Some(2048)), row(None)]);
}

#[test]
fn unlimited_limit_survives_as_null() {
    let bytes = OsCgroupMemory::encode(&[row(None)]).expect("encode");
    let decoded = OsCgroupMemory::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].max, None);
}

#[test]
fn v2_roundtrip() {
    crate::assert_roundtrips(&[OsCgroupMemoryV2 {
        ts: Ts(1),
        cgroup_path: StrId(10),
        current: 1024,
        max: Some(2048),
        anon: 100,
        file: 200,
        kernel: 30,
        slab: 20,
        shmem: 64,
        low_events: 1,
        high_events: 2,
        max_events: 3,
        oom_events: 4,
        oom_kill: 5,
        scope: 1,
    }]);
}
