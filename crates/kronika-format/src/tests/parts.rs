use super::*;

/// A minimal valid part: magic + one tiny section + catalog + tail.
fn sample_part() -> Vec<u8> {
    let section = *b"data";
    let mut part = Vec::new();
    part.extend_from_slice(&MAGIC);
    part.extend_from_slice(&section);
    let catalog = Catalog {
        entries: vec![Entry {
            type_id: 1_006_001,
            flags: 0,
            offset: 4,
            len: section.len() as u64,
            rows: 1,
            crc32c: crc32c(&section),
        }],
        min_ts: 1,
        max_ts: 2,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    };
    part.extend_from_slice(&catalog.encode());
    part
}

fn frame(part: &[u8]) -> Vec<u8> {
    let mut out = FrameHeader {
        part_len: part.len() as u64,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(part);
    out
}

const fn small_limits() -> JournalLimits {
    JournalLimits { max_part_len: 4096 }
}

#[test]
fn frame_header_layout_is_byte_exact() {
    let encoded = FrameHeader { part_len: 88 }.encode();
    assert_eq!(&encoded[..4], b"ZMSP");
    assert_eq!(&encoded[4..12], &88_u64.to_le_bytes());
    // The CRC pins the covered range: magic + length, little-endian.
    assert_eq!(
        &encoded[12..],
        &crc32c(&encoded[..12]).to_le_bytes(),
        "header crc covers exactly the first 12 bytes"
    );
    assert_eq!(
        FrameHeader::decode(encoded),
        Ok(FrameHeader { part_len: 88 })
    );
}

#[test]
fn frame_header_rejects_damage() {
    let mut bytes = FrameHeader { part_len: 7 }.encode();
    bytes[0] ^= 0xFF;
    assert!(matches!(
        FrameHeader::decode(bytes),
        Err(FrameError::BadMagic { .. })
    ));

    let mut bytes = FrameHeader { part_len: 7 }.encode();
    bytes[5] ^= 0x01;
    assert!(matches!(
        FrameHeader::decode(bytes),
        Err(FrameError::BadCrc { .. })
    ));
}

#[test]
fn validates_a_real_part_and_catches_section_corruption() {
    let part = sample_part();
    let catalog = validate_part(&part).expect("sample part is valid");
    assert_eq!(catalog.entries.len(), 1);

    // Corrupting the section body is caught by the section CRC even
    // though the catalog itself is intact.
    let mut corrupted = part;
    corrupted[5] ^= 0x01;
    assert!(matches!(
        validate_part(&corrupted),
        Err(PartError::SectionCrc { .. })
    ));
}

#[test]
fn catalog_validation_skips_section_body_crc() {
    // A part whose body is corrupt but whose catalog is intact: the full
    // check rejects it, the catalog-only check accepts it (the reader
    // re-verifies bodies on decode).
    let mut part = sample_part();
    part[5] ^= 0x01;
    assert!(matches!(
        validate_part(&part),
        Err(PartError::SectionCrc { .. })
    ));
    assert!(validate_part_catalog(&part).is_ok());
    // The catalog-only check still rejects a structural failure.
    let mut bad_magic = sample_part();
    bad_magic[0] ^= 0xFF;
    assert!(matches!(
        validate_part_catalog(&bad_magic),
        Err(PartError::BadMagic { .. })
    ));
}

#[test]
fn part_validation_rejects_duplicate_section_types() {
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
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
        validate_part_catalog(&part),
        Err(PartError::Layout(_))
    ));
}

#[test]
fn part_validation_accepts_canonical_dictionary_tail() {
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 1,
                body: b"data",
            },
            SectionInput {
                type_id: 3_001_001,
                rows: 1,
                body: b"strings",
            },
            SectionInput {
                type_id: 3_002_001,
                rows: 1,
                body: b"blobs",
            },
        ],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );

    assert!(validate_part(&part).is_ok());
}

#[test]
fn build_part_round_trips_through_validate_part() {
    let first: &[u8] = b"section-one-body";
    let second: &[u8] = b"second";
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 3,
                body: first,
            },
            SectionInput {
                type_id: 1_021_001,
                rows: 1,
                body: second,
            },
        ],
        PartMeta {
            min_ts: 100,
            max_ts: 900,
        },
    );

    let catalog = validate_part(&part).expect("built part is valid");
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!((catalog.min_ts, catalog.max_ts), (100, 900));
    assert_eq!(catalog.entries[0].type_id, 1_006_001);
    assert_eq!(catalog.entries[0].rows, 3);
    assert_eq!(catalog.entries[0].offset, MAGIC.len() as u64);

    // Each recorded (offset, len) slices back to the exact body that went in.
    for (entry, body) in catalog.entries.iter().zip([first, second]) {
        let start = usize::try_from(entry.offset).expect("offset fits usize");
        let len = usize::try_from(entry.len).expect("len fits usize");
        assert_eq!(&part[start..start + len], body);
    }
}

#[test]
fn build_part_appends_the_exact_catalog_encoding() {
    let bodies: [&[u8]; 2] = [b"first", b"second"];
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 3,
                body: bodies[0],
            },
            SectionInput {
                type_id: 1_021_001,
                rows: 4,
                body: bodies[1],
            },
        ],
        PartMeta {
            min_ts: 100,
            max_ts: 900,
        },
    );
    let first_offset = MAGIC.len() as u64;
    let catalog = Catalog {
        entries: vec![
            Entry {
                type_id: 1_006_001,
                flags: 0,
                offset: first_offset,
                len: bodies[0].len() as u64,
                rows: 3,
                crc32c: crc32c(bodies[0]),
            },
            Entry {
                type_id: 1_021_001,
                flags: 0,
                offset: first_offset + bodies[0].len() as u64,
                len: bodies[1].len() as u64,
                rows: 4,
                crc32c: crc32c(bodies[1]),
            },
        ],
        min_ts: 100,
        max_ts: 900,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    };
    assert!(part.ends_with(&catalog.encode()));
}

#[test]
fn build_part_accepts_no_sections() {
    let part = build_part(
        &[],
        PartMeta {
            min_ts: 0,
            max_ts: 0,
        },
    );
    let catalog = validate_part(&part).expect("empty part is valid");
    assert!(catalog.entries.is_empty());
}

/// The journal scanner over an in-memory buffer.
fn scan(bytes: &[u8]) -> ScanReport {
    scan_journal_streaming_strict_from(&bytes, 0, small_limits(), 64).expect("a buffer reads")
}

#[test]
fn a_built_part_passes_the_journal_scan() {
    let part = build_part(
        &[SectionInput {
            type_id: 1_006_001,
            rows: 1,
            body: b"data",
        }],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );
    let report = scan(&frame(&part));
    assert_eq!(report.parts.len(), 1);
}

#[test]
fn clean_journal_scans_clean() {
    let part = sample_part();
    let mut journal = Vec::new();
    journal.extend_from_slice(&frame(&part));
    journal.extend_from_slice(&frame(&part));

    let report = scan(&journal);
    assert_eq!(report.parts.len(), 2);
    assert_eq!(report.valid_len, journal.len());
    for part_ref in &report.parts {
        let body = &journal[part_ref.offset..part_ref.offset + part_ref.len];
        assert_eq!(body, part.as_slice());
    }
}

#[test]
fn incomplete_final_frame_keeps_the_valid_prefix() {
    let part = sample_part();
    let mut journal = frame(&part);
    let full = frame(&part);
    journal.extend_from_slice(&full[..full.len() - 3]);

    let report = scan(&journal);
    assert_eq!(report.parts.len(), 1);
    assert_eq!(
        report.valid_len,
        frame(&part).len(),
        "truncation point is the end of the last valid frame"
    );
}

#[test]
fn a_corrupt_frame_ends_the_scan_even_with_valid_frames_behind_it() {
    let part = sample_part();
    let one = frame(&part);
    let mut journal = Vec::new();
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    journal[one.len()] ^= 0xFF;

    let report = scan(&journal);
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.valid_len, one.len());
}

#[test]
fn a_corrupt_part_body_under_an_intact_header_ends_the_scan() {
    let part = sample_part();
    let one = frame(&part);
    let mut journal = Vec::new();
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    journal[one.len() + FRAME_HEADER_LEN + 5] ^= 0x01;

    let report = scan(&journal);
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.valid_len, one.len());
}

#[test]
fn oversized_length_claim_ends_the_scan() {
    let part = sample_part();
    let mut journal = frame(&part);
    // A frame claiming a part over the configured limit, with a valid CRC.
    journal.extend_from_slice(
        &FrameHeader {
            part_len: small_limits().max_part_len + 1,
        }
        .encode(),
    );

    let report = scan(&journal);
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.valid_len, frame(&part).len());
}
