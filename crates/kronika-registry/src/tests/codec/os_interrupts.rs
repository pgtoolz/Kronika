use super::OsInterrupts;
use crate::{Section, StrId, Ts, contract::lint};

fn row(irq: u64, device: Option<u64>) -> OsInterrupts {
    OsInterrupts {
        ts: Ts(10),
        irq: StrId(irq),
        device: device.map(StrId),
        count: 1_234_567,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsInterrupts::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let contract = OsInterrupts::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_114_001);
    assert_eq!(contract.sort_key, ["irq", "ts"]);
}

#[test]
fn roundtrip_keeps_the_absent_device() {
    crate::assert_roundtrips(&[row(1, Some(2)), row(3, None)]);
}
