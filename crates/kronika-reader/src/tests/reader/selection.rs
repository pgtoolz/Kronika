use super::*;

#[test]
fn range_discovery_checks_bodies_only_after_selection() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let first_address = address(SEGMENT_ID);
    let second_address = address(SEGMENT_ID + 1);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, first_address.id, 100, b"first");
    write_segment(&journal, &owner, first_address).expect("publish first");
    journal.reset().expect("reset after first");
    append_text_window(&mut journal, second_address.id, 200, b"second");
    write_segment(&journal, &owner, second_address).expect("publish second");
    journal.reset().expect("leave no active segment");

    let second_path = zms_path(directory.path(), second_address);
    let mut bytes = std::fs::read(&second_path).expect("read second segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&second_path, bytes).expect("damage one selected body");

    let reader = Reader::open(directory.path()).expect("open reader");
    let first = reader.segments(100..=100).expect("select first only");
    assert_eq!(first.segments.len(), 1);
    assert!(
        first.warnings.is_empty(),
        "unselected body must stay unread"
    );

    let second = reader.segments(200..=200).expect("select damaged second");
    assert!(second.segments.is_empty());
    assert_eq!(second.warnings.len(), 1);
    assert!(matches!(
        second.warnings[0].reason,
        kronika_store::StoreWarningReason::InvalidZms(
            kronika_store::InvalidZmsReason::SectionChecksum
        )
    ));

    let catalog = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments(200..=200)
        .expect("catalog-only discovery");
    assert_eq!(catalog.segments.len(), 1, "catalog remains discoverable");
    assert!(catalog.warnings.is_empty());
    let selected = reader
        .open_segment(&catalog.segments[0])
        .expect("open selected catalog");
    assert!(
        selected.rows(OsTopology::CONTRACT.type_id.get()).is_err(),
        "the production row path must reject the damaged selected body"
    );

    let exact = reader
        .catalog_segment(second_address.id.get())
        .expect("exact catalog discovery");
    assert_eq!(
        exact
            .segments
            .iter()
            .map(crate::SegmentRef::id)
            .collect::<Vec<_>>(),
        [second_address.id.get()]
    );
    assert!(
        reader
            .catalog_segment(second_address.id.get() + 1)
            .expect("missing exact catalog")
            .segments
            .is_empty()
    );

    let discovery = reader.catalog_discovery().expect("compact discovery");
    assert_eq!(
        discovery.ranges().collect::<Vec<_>>(),
        vec![(100, 100), (200, 200)]
    );
    let selected = discovery
        .segments(200..=200)
        .expect("materialize selected catalog");
    assert_eq!(
        selected
            .segments
            .iter()
            .map(crate::SegmentRef::id)
            .collect::<Vec<_>>(),
        [second_address.id.get()]
    );

    let with_predecessor = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments_with_predecessor(200..=200)
        .expect("bounded catalogs with predecessor");
    assert_eq!(
        with_predecessor
            .segments
            .iter()
            .map(crate::SegmentRef::id)
            .collect::<Vec<_>>(),
        vec![first_address.id.get(), second_address.id.get()]
    );
}
