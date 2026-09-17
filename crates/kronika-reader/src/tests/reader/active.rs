use super::*;

#[test]
fn a_segment_reference_cannot_cross_reader_roots() {
    let first_directory = tempfile::tempdir().expect("first tempdir");
    let first_owner = writer(&first_directory);
    let address = address(SEGMENT_ID);
    let mut journal =
        Journal::open(&first_owner, JournalConfig::default()).expect("open first journal");
    append_text_window(&mut journal, address.id, 100, b"first root");
    let first_reader = Reader::open(first_directory.path()).expect("open first reader");
    let listing = first_reader.segments(..).expect("list first root");

    let second_directory = tempfile::tempdir().expect("second tempdir");
    let _second_owner = writer(&second_directory);
    let second_reader = Reader::open(second_directory.path()).expect("open second reader");
    let error = second_reader
        .open_segment(&listing.segments[0])
        .expect_err("a reference is bound to the reader that listed it");
    assert!(matches!(
        error,
        ReaderError::Io(ref source) if source.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[test]
fn active_read_keeps_its_prefix_and_next_read_gets_later_deltas() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let first_id = append_text_window(&mut journal, address.id, 100, b"first");
    let first_prefix_bytes = std::fs::metadata(directory.path().join("active.wal"))
        .expect("active journal metadata")
        .len();

    let reader = Reader::open(directory.path()).expect("open reader");
    let first_listing = reader.segments(..).expect("capture first prefix");
    assert_eq!(first_listing.segments.len(), 1);
    assert!(
        reader
            .segments(..100)
            .expect("range before")
            .segments
            .is_empty()
    );
    assert!(
        reader
            .segments(101..)
            .expect("range after")
            .segments
            .is_empty()
    );

    let second_id = append_text_window(&mut journal, address.id, 200, b"second");
    let first = reader
        .open_segment(&first_listing.segments[0])
        .expect("open captured prefix after append");
    assert_eq!(first.captured_bytes(), first_prefix_bytes);
    assert_eq!(
        first
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        1
    );
    let first_dictionary = first.dictionary().expect("first dictionary");
    assert_eq!(
        first_dictionary.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(first_dictionary.resolve(second_id.get()), None);

    let second = one_segment(&reader);
    assert_eq!(
        second
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        2
    );
    let second_dictionary = second.dictionary().expect("complete dictionary");
    assert_eq!(
        second_dictionary.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(
        second_dictionary.resolve(second_id.get()),
        Some(Resolved::Str(b"second"))
    );
    let selected = second
        .dictionary_for(&HashSet::from([first_id.get()]))
        .expect("select an id from the older dictionary delta");
    assert_eq!(
        selected.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(selected.resolve(second_id.get()), None);
    assert!(
        reader
            .segments(201..)
            .expect("after active")
            .segments
            .is_empty()
    );
}

#[test]
fn an_active_reference_can_be_pinned_to_an_earlier_committed_position() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");

    let reader = Reader::open(directory.path()).expect("open reader");
    let first = reader.segments(..).expect("capture first prefix");
    let position = first.segments[0]
        .active_position()
        .expect("active position");

    append_text_window(&mut journal, address.id, 200, b"second");
    let latest = reader.segments(..).expect("capture latest prefix");
    let pinned = latest.segments[0]
        .at_active_position(position)
        .expect("pin earlier frame boundary");
    assert_eq!(pinned.active_position(), Some(position));
    let segment = reader.open_segment(&pinned).expect("open pinned prefix");
    assert_eq!(
        segment
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("pinned rows")
            .len(),
        1
    );
}

#[test]
fn projected_visit_keeps_stable_active_ordinals_and_stops_at_its_limit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");
    append_text_window(&mut journal, address.id, 200, b"second");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let mut rows = Vec::new();
    let visited = segment
        .visit_rows(
            OsTopology::CONTRACT.type_id.get(),
            &["cpu_id"],
            1,
            1,
            |ordinal, row| {
                rows.push((ordinal, row));
                true
            },
        )
        .expect("visit projected row");
    assert_eq!(visited, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 1);
    assert_eq!(rows[0].1.get("cpu_id"), Some(&Cell::I32(200)));
    assert_eq!(rows[0].1.get("ts"), Some(&Cell::Null));
}

#[test]
fn batch_visit_keeps_full_schema_and_concatenates_active_part_ordinals() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");
    append_text_window(&mut journal, address.id, 200, b"second");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let mut ordinals = Vec::new();
    let mut cpu_ids: Vec<i32> = Vec::new();
    let visited = segment
        .visit_batches(
            OsTopology::CONTRACT.type_id.get(),
            None,
            0,
            usize::MAX,
            |ordinal, batch| {
                assert_eq!(batch.num_columns(), OsTopology::CONTRACT.columns.len());
                assert_eq!(
                    batch.schema(),
                    kronika_registry::arrow_schema(&OsTopology::CONTRACT)
                );
                ordinals.push(ordinal);
                let values = batch
                    .column_by_name("cpu_id")
                    .expect("cpu_id column")
                    .as_any()
                    .downcast_ref::<Int32Array>()
                    .expect("cpu_id is Int32");
                cpu_ids.extend(values.values());
                true
            },
        )
        .expect("visit batches");
    assert_eq!(visited, 2);
    assert_eq!(ordinals, [0, 1]);
    assert_eq!(cpu_ids, [100, 200]);
}

#[test]
fn projected_visitors_stop_before_reading_a_later_damaged_part() {
    use std::os::unix::fs::FileExt as _;

    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");
    append_text_window(&mut journal, address.id, 200, b"second");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let scan = kronika_store::LocalDir::open(directory.path())
        .expect("open store")
        .scan_journal()
        .expect("capture part ranges");
    let second = &scan.active[1];
    let type_id = OsTopology::CONTRACT.type_id.get();
    let section = second
        .catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == type_id)
        .expect("topology section");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(directory.path().join("active.wal"))
        .expect("open active journal");
    file.write_all_at(&[0], second.part.offset as u64 + section.offset)
        .expect("damage later Parquet body");

    assert_eq!(
        segment
            .visit_rows(type_id, &["cpu_id"], 0, usize::MAX, |ordinal, _row| {
                assert_eq!(ordinal, 0);
                false
            })
            .expect("row callback stops before damaged part"),
        1
    );
    assert_eq!(
        segment
            .visit_batches(type_id, None, 0, usize::MAX, |ordinal, _batch| {
                assert_eq!(ordinal, 0);
                false
            })
            .expect("batch callback stops before damaged part"),
        1
    );
    assert_eq!(
        segment
            .visit_rows(type_id, &["cpu_id"], 0, 1, |_ordinal, _row| true)
            .expect("row limit skips damaged part"),
        1
    );
    assert_eq!(
        segment
            .visit_batches(type_id, None, 0, 1, |_ordinal, _batch| true)
            .expect("batch limit skips damaged part"),
        1
    );
    assert!(
        segment
            .visit_rows(type_id, &["cpu_id"], 1, 1, |_ordinal, _row| true)
            .is_err()
    );
    assert!(
        segment
            .visit_batches(type_id, None, 1, 1, |_ordinal, _batch| true)
            .is_err()
    );
}
