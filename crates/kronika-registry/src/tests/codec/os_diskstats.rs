use super::OsDiskstats;
use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

fn full_row(ts: i64, major: i32, minor: i32) -> OsDiskstats {
    OsDiskstats {
        ts: Ts(ts),
        major,
        minor,
        device: StrId(42),
        reads: 100,
        r_merged: 2,
        read_sectors: 3000,
        read_time_ms: 40,
        writes: 200,
        w_merged: 5,
        write_sectors: 6000,
        write_time_ms: 70,
        io_in_progress: 1,
        io_time_ms: 800,
        io_weighted_time_ms: 900,
        discards: Some(10),
        d_merged: Some(11),
        discard_sectors: Some(12),
        discard_time_ms: Some(13),
        flushes: Some(14),
        flush_time_ms: Some(15),
        scope: 0,
    }
}

fn legacy_row(ts: i64) -> OsDiskstats {
    OsDiskstats {
        ts: Ts(ts),
        major: 259,
        minor: 0,
        device: StrId(7),
        reads: 1,
        r_merged: 0,
        read_sectors: 8,
        read_time_ms: 2,
        writes: 3,
        w_merged: 0,
        write_sectors: 24,
        write_time_ms: 4,
        io_in_progress: 0,
        io_time_ms: 6,
        io_weighted_time_ms: 6,
        discards: None,
        d_merged: None,
        discard_sectors: None,
        discard_time_ms: None,
        flushes: None,
        flush_time_ms: None,
        scope: 0,
    }
}

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsDiskstats::CONTRACT]), Ok(()));
}

#[test]
fn contract_shape() {
    let c = OsDiskstats::CONTRACT;
    assert_eq!(c.type_id.get(), 1_108_001);
    assert_eq!(c.sort_key, ["major", "minor", "ts"]);
    assert_eq!(c.identity, ["major", "minor"]);
}

#[test]
fn roundtrip() {
    crate::assert_roundtrips(&[full_row(1_000, 8, 0), legacy_row(2_000)]);
}

#[test]
fn nulls_survive_distinct_from_zero() {
    let bytes = OsDiskstats::encode(&[legacy_row(5)]).expect("encode");
    let decoded = OsDiskstats::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded[0].discards, None);
    assert_eq!(decoded[0].d_merged, None);
    assert_eq!(decoded[0].discard_sectors, None);
    assert_eq!(decoded[0].discard_time_ms, None);
    assert_eq!(decoded[0].flushes, None);
    assert_eq!(decoded[0].flush_time_ms, None);
}
