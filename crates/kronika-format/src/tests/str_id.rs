use super::StrId;

/// The canonical `XXH3_64bits` check value for empty input from the
/// xxHash reference implementation. Catches accidentally swapping the
/// algorithm (e.g. plain xxh64) the same way the CRC32C known vector
/// does for checksums.
#[test]
fn known_vector_empty_input() {
    let id = StrId::of(b"").expect("empty input must hash to a non-zero id");
    assert_eq!(id.get(), 0x2D06_8005_38D3_94C2);
}

#[test]
fn raw_roundtrip_and_zero_sentinel() {
    let id = StrId::of(b"pg_stat_activity").expect("non-zero id");
    assert_eq!(StrId::from_raw(id.get()), Some(id));
    assert_eq!(StrId::from_raw(0), None);
}
