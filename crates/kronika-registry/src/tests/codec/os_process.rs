use super::OsProcess;
use crate::test_support::process_row as test_row;
use crate::{Section, Unit, VerifiedSection, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsProcess::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsProcess::CONTRACT;
    assert_eq!(c.type_id.get(), 1_100_001);
    assert_eq!(c.sort_key, ["pid", "ts"]);
    assert_eq!(c.identity, ["pid"]);
    for name in ["rchar", "wchar", "read_bytes", "write_bytes"] {
        assert_eq!(
            c.column(name).and_then(|column| column.unit),
            Some(Unit::Bytes)
        );
    }
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[test_row(1, 10, true), test_row(2, 11, false)]);
}

#[test]
fn io_nulls_survive() {
    let bytes = OsProcess::encode(&[test_row(1, 10, false)]).expect("encode");
    let decoded = OsProcess::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].syscr, None);
    assert_eq!(decoded[0].read_bytes, None);
    assert_eq!(decoded[0].cancelled_write_bytes, None);
}
