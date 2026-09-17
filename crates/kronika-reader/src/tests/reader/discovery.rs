use super::*;

#[test]
fn finished_segment_wins_over_the_same_active_generation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"one row");
    write_segment(&journal, &owner, address).expect("publish finished segment");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    assert_eq!(segment.kind(), SegmentKind::Finished);
    assert_eq!(
        segment.source_label(),
        zms_path(directory.path(), address).display().to_string()
    );
    assert_eq!(
        segment.captured_bytes(),
        std::fs::metadata(zms_path(directory.path(), address))
            .expect("finished segment metadata")
            .len()
    );
    assert_eq!(
        segment
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        1,
        "the active generation must not duplicate the finished segment"
    );

    let predecessor = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments_with_predecessor(200..=200)
        .expect("select canonical predecessor");
    assert_eq!(predecessor.segments.len(), 1);
    let predecessor = reader
        .open_segment(&predecessor.segments[0])
        .expect("open canonical predecessor");
    assert_eq!(predecessor.kind(), SegmentKind::Finished);
}

#[test]
fn active_segment_can_be_the_closest_catalog_predecessor() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let finished_address = address(SEGMENT_ID);
    let active_address = address(SEGMENT_ID + 1);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, finished_address.id, 100, b"finished");
    write_segment(&journal, &owner, finished_address).expect("publish finished segment");
    journal.reset().expect("start the active generation");
    append_text_window(&mut journal, active_address.id, 200, b"active");

    let reader = Reader::open(directory.path()).expect("open reader");
    let discovery = reader.catalog_discovery().expect("capture catalog scan");
    assert_eq!(
        discovery.ranges().collect::<Vec<_>>(),
        vec![(100, 100), (200, 200)]
    );
    let listing = discovery
        .segments_with_predecessor(300..=300)
        .expect("materialize active predecessor from captured scan");
    assert_eq!(
        listing
            .segments
            .iter()
            .map(crate::SegmentRef::id)
            .collect::<Vec<_>>(),
        [active_address.id.get()]
    );
    assert!(listing.segments[0].active_position().is_some());
}

#[test]
fn compatible_catalog_predecessor_skips_a_sectionless_segment() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let predecessor = address(SEGMENT_ID);
    let sectionless = address(SEGMENT_ID + 1);
    let current = address(SEGMENT_ID + 2);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, predecessor.id, 100, b"predecessor");
    write_segment(&journal, &owner, predecessor).expect("publish predecessor");
    journal.reset().expect("start sectionless generation");
    append_cpu_window(&mut journal, sectionless.id, 200);
    write_segment(&journal, &owner, sectionless).expect("publish sectionless segment");
    journal.reset().expect("start current generation");
    append_text_window(&mut journal, current.id, 300, b"current");
    write_segment(&journal, &owner, current).expect("publish current segment");

    let reader = Reader::open(directory.path()).expect("open reader");
    let listing = reader
        .catalog_discovery()
        .expect("capture catalog scan")
        .segments_with_predecessors_for(300..=300, &[OsTopology::CONTRACT.type_id.get()])
        .expect("select compatible predecessor");
    assert_eq!(
        listing
            .segments
            .iter()
            .map(crate::SegmentRef::id)
            .collect::<Vec<_>>(),
        [predecessor.id.get(), current.id.get()]
    );
}

#[test]
fn damaged_finished_segment_does_not_hide_the_same_valid_active_generation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"one row");
    write_segment(&journal, &owner, address).expect("publish finished segment");

    let path = zms_path(directory.path(), address);
    let mut bytes = std::fs::read(&path).expect("read finished segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&path, bytes).expect("damage finished section body");

    let reader = Reader::open(directory.path()).expect("open reader");
    let listing = reader.segments(..).expect("list with body validation");
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(listing.warnings.len(), 1);
    let segment = reader
        .open_segment(&listing.segments[0])
        .expect("open active fallback");
    assert_eq!(segment.kind(), SegmentKind::Active);
    assert_eq!(
        segment.source_label(),
        directory.path().join("active.wal").display().to_string()
    );
}
