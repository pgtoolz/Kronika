//! Reading a finished segment catalog and rejecting a malformed one.

use super::*;

#[test]
fn read_catalog_too_small_buffer_shorter_than_tail() {
    // A buffer shorter than TAIL_INDEX_LEN cannot hold a tail index.
    let buf: &[u8] = &[0_u8; TAIL_INDEX_LEN - 1];
    assert!(
        matches!(read_catalog(&buf), Err(StoreError::TooSmall)),
        "buffer shorter than tail index must return TooSmall"
    );
}

#[test]
fn read_catalog_reports_bad_tail_index_magic() {
    // Exactly TAIL_INDEX_LEN bytes with wrong magic retains a precise
    // typed tail-index failure.
    let buf = [0_u8; TAIL_INDEX_LEN];
    assert!(
        matches!(read_catalog(&buf.as_slice()), Err(StoreError::TailIndex(_))),
        "tail with wrong magic must return TailIndex"
    );
}

#[test]
fn read_catalog_bad_catalog_len_exceeds_max() {
    // Tail index with catalog_len > MAX_CATALOG_BYTES (64 MiB).
    // Build tail manually: catalog_len as u32 LE + MAGIC.
    // MAX_CATALOG_BYTES = 64 MiB = 0x0400_0000; adding 1 gives 0x0400_0001, which fits u32.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "MAX_CATALOG_BYTES + 1 = 64 MiB + 1 < u32::MAX; truncation is impossible"
    )]
    let huge_len: u32 = (MAX_CATALOG_BYTES + 1) as u32;
    let mut buf = vec![0_u8; TAIL_INDEX_LEN + 100];
    let tail_at = buf.len() - TAIL_INDEX_LEN;
    buf[tail_at..tail_at + 4].copy_from_slice(&huge_len.to_le_bytes());
    buf[tail_at + 4..tail_at + 8].copy_from_slice(&MAGIC);
    assert!(
        matches!(
            read_catalog(&buf.as_slice()),
            Err(StoreError::BadCatalogLen)
        ),
        "catalog_len > MAX_CATALOG_BYTES must return BadCatalogLen"
    );
}

#[test]
fn read_catalog_bad_catalog_len_catalog_overlaps_magic() {
    // catalog_len so large that catalog_at would land before the magic.
    // The file is: MAGIC(4) + tail_index(8) = 12 bytes.
    // Set catalog_len = 9 so catalog_at = 12 - 8 - 9 = -5 (underflow → BadCatalogLen).
    let catalog_len: u32 = 9;
    let mut buf = vec![0_u8; 12];
    buf[0..4].copy_from_slice(&MAGIC);
    let tail_at = 4;
    buf[tail_at..tail_at + 4].copy_from_slice(&catalog_len.to_le_bytes());
    buf[tail_at + 4..tail_at + 8].copy_from_slice(&MAGIC);
    assert!(
        matches!(
            read_catalog(&buf.as_slice()),
            Err(StoreError::BadCatalogLen)
        ),
        "catalog extending past magic must return BadCatalogLen"
    );
}

#[test]
fn read_catalog_catalog_decode_error() {
    // Valid tail + catalog bytes that fail Catalog::decode (all-zeroes meta has bad CRC).
    // File: MAGIC(4) | catalog_block(META_LEN=40, all zeroes) | tail(8)
    let catalog_len = u32::try_from(META_LEN).expect("META_LEN fits u32");
    let total = MAGIC.len() + META_LEN + TAIL_INDEX_LEN;
    let mut buf = vec![0_u8; total];
    buf[0..4].copy_from_slice(&MAGIC);
    let tail_at = MAGIC.len() + META_LEN;
    buf[tail_at..tail_at + 4].copy_from_slice(&catalog_len.to_le_bytes());
    buf[tail_at + 4..tail_at + 8].copy_from_slice(&MAGIC);
    // catalog_at = MAGIC.len() = 4, catalog block is all zeroes — CRC mismatch.
    assert!(
        matches!(read_catalog(&buf.as_slice()), Err(StoreError::Catalog(_))),
        "corrupt catalog block must return Catalog(DecodeError)"
    );
}

#[test]
fn read_catalog_bad_magic() {
    // Valid tail + valid catalog, but byte 0 is not MAGIC.
    let mut buf = minimal_part_with_version(FORMAT_VERSION);
    buf[0] ^= 0xFF; // corrupt first byte
    assert!(
        matches!(read_catalog(&buf.as_slice()), Err(StoreError::BadMagic)),
        "wrong magic at offset 0 must return BadMagic"
    );
}

#[test]
fn read_catalog_unsupported_format_version() {
    // Valid part except format_version != FORMAT_VERSION.
    // Patch format_version to 99, then recompute catalog CRC.
    let mut buf = minimal_part_with_version(FORMAT_VERSION);
    let fv_at = format_version_offset(&buf);
    buf[fv_at..fv_at + 4].copy_from_slice(&99_u32.to_le_bytes());
    repatch_catalog_crc(&mut buf);
    assert!(
        matches!(
            read_catalog(&buf.as_slice()),
            Err(StoreError::UnsupportedFormat { version: 99 })
        ),
        "unknown format_version must return UnsupportedFormat"
    );
}

#[test]
fn read_catalog_out_of_bounds_entry() {
    // Build a part with one section, then patch that entry's offset to point
    // into the catalog block (past catalog_at), triggering OutOfBounds.
    let section_body = b"data";
    let mut buf = build_part(
        &[SectionInput {
            type_id: 1_006_001,
            rows: 1,
            body: section_body,
        }],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );
    // Entry layout in catalog block: type_id(4) flags(4) offset(8) len(8) rows(4) crc32c(4)
    // offset field starts at byte 8 within the first entry.
    let cat_start = catalog_offset(&buf);
    let entry_offset_field = cat_start + 8;
    // Set offset to a value past catalog_start (i.e., into the catalog block itself).
    let bad_offset = cat_start as u64 + 1;
    buf[entry_offset_field..entry_offset_field + 8].copy_from_slice(&bad_offset.to_le_bytes());
    // Recompute catalog CRC so Catalog::decode succeeds.
    repatch_catalog_crc(&mut buf);
    assert!(
        matches!(
            read_catalog(&buf.as_slice()),
            Err(StoreError::SectionLayout(_))
        ),
        "entry pointing into catalog block must fail physical layout validation"
    );
}

#[test]
fn read_catalog_rejects_noncanonical_section_order() {
    let buf = build_part(
        &[
            SectionInput {
                type_id: 1_021_001,
                rows: 1,
                body: b"first",
            },
            SectionInput {
                type_id: 1_006_001,
                rows: 1,
                body: b"second",
            },
        ],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );

    assert!(matches!(
        read_catalog(&buf.as_slice()),
        Err(StoreError::SectionLayout(_))
    ));
}

#[test]
fn read_catalog_happy_path() {
    // Confirm a correctly built part round-trips through read_catalog.
    let buf = part(1000);
    let catalog = read_catalog(&buf.as_slice()).expect("valid part must decode");
    assert_eq!(catalog.min_ts, 1000);
}
