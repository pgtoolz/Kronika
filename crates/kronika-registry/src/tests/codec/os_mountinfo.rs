use super::OsMountinfo;
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn full_row(ts: i64, major: i32, minor: i32) -> OsMountinfo {
    OsMountinfo {
        ts: Ts(ts),
        major,
        minor,
        mount_point: StrId(10),
        root: StrId(13),
        fstype: StrId(11),
        source: StrId(12),
        is_k8s_infra: false,
        total_bytes: Some(10_000_000_000),
        free_bytes: Some(5_000_000_000),
        total_inodes: Some(1_000_000),
        available_inodes: Some(500_000),
        scope: 0,
    }
}

fn no_space_row(ts: i64) -> OsMountinfo {
    OsMountinfo {
        ts: Ts(ts),
        major: 0,
        minor: 35,
        mount_point: StrId(20),
        root: StrId(23),
        fstype: StrId(21),
        source: StrId(22),
        is_k8s_infra: true,
        total_bytes: None,
        free_bytes: None,
        total_inodes: None,
        available_inodes: None,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsMountinfo::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsMountinfo::CONTRACT;
    assert_eq!(c.type_id.get(), 1_112_002);
    assert_eq!(c.sort_key, ["major", "minor", "mount_point", "ts"]);
    assert_eq!(c.identity, ["major", "minor", "mount_point"]);
}

#[test]
fn roundtrip() {
    // Input in sort-key order: (major=0,...) before (major=8,...).
    crate::assert_roundtrips(&[no_space_row(2_000), full_row(1_000, 8, 1)]);
}

#[test]
fn nulls_survive_distinct_from_zero() {
    let bytes = OsMountinfo::encode(&[no_space_row(5)]).expect("encode");
    let decoded = OsMountinfo::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(
        decoded[0].total_bytes, None,
        "total_bytes must be None, not 0"
    );
    assert_eq!(
        decoded[0].free_bytes, None,
        "free_bytes must be None, not 0"
    );
    assert_eq!(decoded[0].total_inodes, None);
    assert_eq!(decoded[0].available_inodes, None);
}

#[test]
fn zero_bytes_is_distinct_from_null() {
    let zero_row = OsMountinfo {
        total_bytes: Some(0),
        free_bytes: Some(0),
        total_inodes: Some(0),
        available_inodes: Some(0),
        ..full_row(10, 8, 2)
    };
    let bytes = OsMountinfo::encode(&[zero_row]).expect("encode");
    let decoded = OsMountinfo::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].total_bytes, Some(0));
    assert_eq!(decoded[0].free_bytes, Some(0));
    assert_eq!(decoded[0].total_inodes, Some(0));
    assert_eq!(decoded[0].available_inodes, Some(0));
}
