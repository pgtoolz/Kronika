use super::*;

fn sample() -> Catalog {
    Catalog {
        entries: vec![Entry {
            type_id: 1_006_001,
            flags: 0,
            offset: 4,
            len: 4,
            rows: 1,
            crc32c: 0x2930_8CF4,
        }],
        min_ts: 1_000_000,
        max_ts: 2_000_000,
        format_version: 1,
        window_count: 7,
    }
}

#[test]
fn catalog_metadata_is_32_bytes() {
    assert_eq!(META_LEN, 32);
}

#[test]
fn old_40_byte_metadata_is_rejected() {
    assert!(matches!(
        Catalog::decode(&[0_u8; 40]),
        Err(DecodeError::BadCatalogLen { actual: 40 })
    ));
}

#[test]
fn tail_index_roundtrip() {
    let tail = TailIndex { catalog_len: 72 };
    assert_eq!(TailIndex::decode(tail.encode()), Ok(tail));
}

#[test]
fn tail_index_rejects_bad_magic() {
    let mut bytes = TailIndex { catalog_len: 72 }.encode();
    bytes[5] ^= 0xFF;
    assert!(matches!(
        TailIndex::decode(bytes),
        Err(DecodeError::BadTailMagic { .. })
    ));
}

#[test]
fn catalog_roundtrip() {
    let catalog = sample();
    let encoded = catalog.encode();
    let body = &encoded[..encoded.len() - TAIL_INDEX_LEN];
    assert_eq!(Catalog::decode(body), Ok(catalog));
}

#[test]
fn streaming_encoder_matches_the_in_memory_encoding() {
    let catalog = sample();
    let expected = catalog.encode();
    let mut streamed = Vec::new();
    catalog
        .write_encoded(&mut streamed)
        .expect("write catalog to memory");
    assert_eq!(streamed, expected);
}

#[test]
fn borrowed_view_matches_owned_decode_without_allocating_entries() {
    let catalog = sample();
    let encoded = catalog.encode();
    let body = &encoded[..encoded.len() - TAIL_INDEX_LEN];
    let view = Catalog::view(body).expect("valid borrowed view");

    assert_eq!(view.min_ts, catalog.min_ts);
    assert_eq!(view.max_ts, catalog.max_ts);
    assert_eq!(view.entry_count, 1);
    assert_eq!(view.format_version, catalog.format_version);
    assert_eq!(view.window_count, catalog.window_count);
    assert_eq!(view.entries().collect::<Vec<_>>(), catalog.entries);
}

#[test]
fn empty_catalog_roundtrip() {
    let catalog = Catalog {
        entries: vec![],
        min_ts: 0,
        max_ts: 0,
        format_version: 1,
        window_count: 0,
    };
    let encoded = catalog.encode();
    let body = &encoded[..encoded.len() - TAIL_INDEX_LEN];
    assert_eq!(Catalog::decode(body), Ok(catalog));
}

#[test]
fn decode_rejects_wrong_length() {
    assert!(matches!(
        Catalog::decode(&[0_u8; META_LEN + 1]),
        Err(DecodeError::BadCatalogLen { .. })
    ));
    assert!(matches!(
        Catalog::decode(&[0_u8; META_LEN - 1]),
        Err(DecodeError::BadCatalogLen { .. })
    ));
}

#[test]
fn decode_rejects_entry_count_mismatch() {
    let encoded = sample().encode();
    let mut body = encoded[..encoded.len() - TAIL_INDEX_LEN].to_vec();
    // Patch entry_count from 1 to 2; offset 16 within meta.
    let at = body.len() - META_LEN + 16;
    body[at..at + 4].copy_from_slice(&2_u32.to_le_bytes());
    assert_eq!(
        Catalog::decode(&body),
        Err(DecodeError::EntryCountMismatch {
            stored: 2,
            derived: 1
        })
    );
}

#[test]
fn decode_rejects_corrupted_byte() {
    let encoded = sample().encode();
    let mut body = encoded[..encoded.len() - TAIL_INDEX_LEN].to_vec();
    body[0] ^= 0x01;
    assert!(matches!(
        Catalog::decode(&body),
        Err(DecodeError::BadCrc { .. })
    ));
}

fn layout_catalog(entries: Vec<Entry>) -> Catalog {
    Catalog {
        entries,
        min_ts: 0,
        max_ts: 0,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    }
}

const fn layout_entry(type_id: u32, offset: u64, len: u64) -> Entry {
    Entry {
        type_id,
        flags: 0,
        offset,
        len,
        rows: 1,
        crc32c: 0,
    }
}

#[test]
fn canonical_layout_accepts_data_then_dictionary_tail() {
    let catalog = layout_catalog(vec![
        layout_entry(1_006_001, 4, 2),
        layout_entry(1_021_001, 6, 3),
        layout_entry(DICT_STRINGS_TYPE_ID, 9, 1),
        layout_entry(DICT_BLOBS_TYPE_ID, 10, 2),
    ]);

    assert_eq!(validate_catalog_layout(&catalog, 12), Ok(()));
}

#[test]
fn canonical_layout_rejects_duplicate_or_misordered_sections() {
    let duplicate = layout_catalog(vec![
        layout_entry(1_006_001, 4, 1),
        layout_entry(1_006_001, 5, 1),
    ]);
    assert!(validate_catalog_layout(&duplicate, 6).is_err());

    let misordered = layout_catalog(vec![
        layout_entry(DICT_STRINGS_TYPE_ID, 4, 1),
        layout_entry(1_006_001, 5, 1),
    ]);
    assert!(validate_catalog_layout(&misordered, 6).is_err());
}

#[test]
fn canonical_layout_rejects_flags_caps_and_noncontiguous_bodies() {
    let mut flagged = layout_catalog(vec![layout_entry(1_006_001, 4, 1)]);
    flagged.entries[0].flags = 1;
    assert!(validate_catalog_layout(&flagged, 5).is_err());

    let oversized = layout_catalog(vec![layout_entry(
        1_006_001,
        4,
        MAX_PHYSICAL_SECTION_BYTES + 1,
    )]);
    assert!(validate_catalog_layout(&oversized, 4 + MAX_PHYSICAL_SECTION_BYTES + 1).is_err());

    let boundary = layout_catalog(vec![layout_entry(1_006_001, 4, MAX_PHYSICAL_SECTION_BYTES)]);
    assert_eq!(
        validate_catalog_layout(&boundary, 4 + MAX_PHYSICAL_SECTION_BYTES),
        Ok(())
    );

    let short = layout_catalog(vec![layout_entry(1_006_001, 5, 1)]);
    assert!(validate_catalog_layout(&short, 6).is_err());

    let trailing = layout_catalog(vec![layout_entry(1_006_001, 4, 1)]);
    assert!(validate_catalog_layout(&trailing, 6).is_err());
}
