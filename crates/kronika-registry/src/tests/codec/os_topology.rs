use super::OsTopology;
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn row(ts: i64, cpu_id: i32) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id,
        model_name: StrId(1),
        mhz_max: Some(3600.0),
        core_id: 0,
        socket_id: 0,
        numa_node: 0,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsTopology::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsTopology::CONTRACT;
    assert_eq!(c.type_id.get(), 1_113_001);
    assert_eq!(c.sort_key, ["cpu_id", "ts"]);
    assert_eq!(c.identity, ["cpu_id"]);
}

#[test]
fn roundtrip() {
    // Input already in sort-key order: (cpu_id=0,ts=1000), (cpu_id=0,ts=2000), (cpu_id=1,ts=1000).
    crate::assert_roundtrips(&[row(1_000, 0), row(2_000, 0), row(1_000, 1)]);
}

#[test]
fn sorts_by_cpu_id_then_ts() {
    let bytes = OsTopology::encode(&[row(2_000, 1), row(1_000, 0), row(1_000, 1)]).expect("encode");
    let decoded = OsTopology::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].cpu_id, 0);
    assert_eq!(decoded[1].cpu_id, 1);
    assert_eq!(decoded[1].ts.0, 1_000);
    assert_eq!(decoded[2].cpu_id, 1);
    assert_eq!(decoded[2].ts.0, 2_000);
}

#[test]
fn null_mhz_max_survives_roundtrip() {
    let value = OsTopology {
        mhz_max: None,
        ..row(1_000, 0)
    };
    let bytes = OsTopology::encode(&[value]).expect("encode");
    let decoded = OsTopology::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].mhz_max, None);
}
