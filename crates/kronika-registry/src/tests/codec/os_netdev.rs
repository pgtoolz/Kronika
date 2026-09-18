use super::OsNetdev;
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn full_row(ts: i64) -> OsNetdev {
    OsNetdev {
        ts: Ts(ts),
        iface: StrId(1),
        rx_bytes: 1000,
        rx_packets: 10,
        rx_errs: 1,
        rx_drop: 2,
        rx_fifo: 3,
        rx_frame: 4,
        rx_compressed: 5,
        rx_multicast: 6,
        tx_bytes: 2000,
        tx_packets: 20,
        tx_errs: 7,
        tx_drop: 8,
        tx_fifo: 9,
        tx_colls: 10,
        tx_carrier: 11,
        tx_compressed: 12,
        speed_mbit: Some(10_000),
        duplex: 2,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsNetdev::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsNetdev::CONTRACT;
    assert_eq!(c.type_id.get(), 1_109_001);
    assert_eq!(c.sort_key, ["iface", "ts"]);
    assert_eq!(c.identity, ["iface"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[full_row(1_000), full_row(2_000)]);
}

#[test]
fn all_sixteen_counters_survive_encode_decode() {
    let bytes = OsNetdev::encode(&[full_row(5)]).expect("encode");
    let decoded = OsNetdev::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    let r = &decoded[0];
    assert_eq!(r.rx_bytes, 1000);
    assert_eq!(r.rx_multicast, 6);
    assert_eq!(r.tx_bytes, 2000);
    assert_eq!(r.tx_compressed, 12);
}
