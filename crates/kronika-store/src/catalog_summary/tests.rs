use super::*;

fn catalog() -> Catalog {
    Catalog {
        entries: vec![
            Entry {
                type_id: 20,
                flags: 0,
                offset: 4,
                len: 3,
                rows: 0,
                crc32c: 1,
            },
            Entry {
                type_id: 10,
                flags: 0,
                offset: 7,
                len: 5,
                rows: 2,
                crc32c: 2,
            },
            Entry {
                type_id: 10,
                flags: 0,
                offset: 12,
                len: 7,
                rows: 3,
                crc32c: 3,
            },
        ],
        min_ts: 100,
        max_ts: 200,
        format_version: FORMAT_VERSION,
        window_count: 3,
    }
}

#[test]
fn encoded_summary_matches_owned_summary() {
    let catalog = catalog();
    let encoded = catalog.encode();
    let catalog_bytes = &encoded[..encoded.len() - TAIL_INDEX_LEN];
    let from_bytes =
        CatalogSummary::from_encoded(catalog_bytes, 19).expect("valid encoded summary");
    let from_catalog =
        CatalogSummary::from_catalog(&catalog, u32::try_from(catalog_bytes.len()).unwrap());

    assert_eq!(from_bytes, from_catalog);
    assert!(from_bytes.may_contain_any_nonempty_type(&[10]));
    assert!(!from_bytes.may_contain_any_nonempty_type(&[20]));
}

#[test]
fn logical_digest_ignores_relocated_offsets_but_layout_digest_does_not() {
    let first = catalog();
    let mut relocated = first.clone();
    for entry in &mut relocated.entries {
        entry.offset += 100;
    }
    let first = CatalogSummary::from_catalog(&first, 136);
    let relocated = CatalogSummary::from_catalog(&relocated, 136);

    assert_eq!(first.logical_digest, relocated.logical_digest);
    assert_ne!(first.layout_digest, relocated.layout_digest);
}

#[test]
fn catalog_digests_include_window_count() {
    let first = catalog();
    let mut more_windows = first.clone();
    more_windows.window_count += 1;

    let first = CatalogSummary::from_catalog(&first, 136);
    let more_windows = CatalogSummary::from_catalog(&more_windows, 136);

    assert_ne!(first.logical_digest, more_windows.logical_digest);
    assert_ne!(first.layout_digest, more_windows.layout_digest);
}

#[test]
fn summary_rejects_entry_outside_the_body_area() {
    let catalog = catalog();
    let encoded = catalog.encode();
    let catalog_bytes = &encoded[..encoded.len() - TAIL_INDEX_LEN];
    assert!(matches!(
        CatalogSummary::from_encoded(catalog_bytes, 18),
        Err(CatalogSummaryError::EntryOutOfBounds { type_id: 10 })
    ));
}

#[test]
fn nonempty_type_filter_has_no_false_negatives() {
    let entries = (1..=10_000).map(|type_id| Entry {
        type_id,
        flags: 0,
        offset: 4,
        len: 0,
        rows: 1,
        crc32c: 0,
    });
    let bloom = nonempty_type_bloom(entries);
    for type_id in 1..=10_000 {
        assert!(bloom_may_contain(&bloom, type_id));
    }
}
