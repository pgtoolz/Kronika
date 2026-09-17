use super::OsCpu;
use crate::{Section, Ts, contract::lint};

fn row(ts: i64, cpu_id: i32) -> OsCpu {
    OsCpu {
        ts: Ts(ts),
        cpu_id,
        user: 1,
        nice: 2,
        system: 3,
        idle: 4,
        iowait: 5,
        irq: 6,
        softirq: 7,
        steal: 8,
        guest: 9,
        guest_nice: 10,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsCpu::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsCpu::CONTRACT;
    assert_eq!(c.type_id.get(), 1_102_001);
    assert_eq!(c.sort_key, ["cpu_id", "ts"]);
    assert_eq!(c.identity, ["cpu_id"]);
}

#[test]
fn encode_sorts_by_cpu_id_then_ts() {
    let bytes = OsCpu::encode(&[row(1_000, 3), row(1_000, -1), row(1_000, 1)]).expect("encode");
    let decoded =
        OsCpu::decode(kronika_registry::VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded.iter().map(|r| r.cpu_id).collect::<Vec<_>>(),
        [-1, 1, 3]
    );
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[row(1_000, -1), row(1_000, 0)]);
}
