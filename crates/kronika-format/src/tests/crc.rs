use super::{Crc32c, crc32c};

/// The canonical CRC32C check value: `crc32c(b"123456789")`.
/// Catches accidentally swapping the polynomial (e.g. plain CRC32).
#[test]
fn known_vector() {
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
}

#[test]
fn empty_input() {
    assert_eq!(crc32c(b""), 0);
}

#[test]
fn incremental_chunks_match_one_shot_crc32c() {
    let mut checksum = Crc32c::new();
    checksum.update(b"123");
    checksum.update(b"456");
    checksum.update(b"789");
    assert_eq!(checksum.finalize(), crc32c(b"123456789"));
}
