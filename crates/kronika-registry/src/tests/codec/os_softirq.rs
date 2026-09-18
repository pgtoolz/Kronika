use super::OsSoftirq;
use crate::{Section, StrId, Ts, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsSoftirq::CONTRACT]), Ok(()));
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[
        OsSoftirq {
            ts: Ts(1),
            vector: StrId(1),
            count: 10,
            scope: 0,
        },
        OsSoftirq {
            ts: Ts(1),
            vector: StrId(2),
            count: 20,
            scope: 0,
        },
    ]);
}
