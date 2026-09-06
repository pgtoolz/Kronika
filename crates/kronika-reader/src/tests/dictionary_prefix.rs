use super::*;

#[test]
fn dictionary_prefix_matches_stored_string_and_blob_bytes_in_both_sources() {
    let directory = tempfile::tempdir().expect("prefix fixture directory");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("prefix journal");
    let prefix = b"/* kronika:";
    let texts = [
        b"/* kronika: collector */ select 1".to_vec(),
        [prefix.as_slice(), &vec![b'x'; DEFAULT_BLOB_THRESHOLD]].concat(),
        [prefix.as_slice(), &vec![b'y'; DEFAULT_TRUNCATE_LIMIT]].concat(),
        b"select '/* kronika:'".to_vec(),
        b"/* KRONIKA: not the prefix */".to_vec(),
        b"/* kronik".to_vec(),
        vec![0xff; DEFAULT_BLOB_THRESHOLD],
    ];
    let mut interner = Interner::new(DictLimits::default());
    let ids: Vec<_> = texts
        .iter()
        .map(|text| interner.intern(text).expect("prefix text"))
        .collect();
    let dictionary = dict::encode(interner.window()).expect("prefix dictionary");
    let mut buffers = SectionBuffers::new();
    for (cpu_id, id) in ids.iter().enumerate() {
        buffers
            .push(topology(100, i32::try_from(cpu_id).expect("cpu id"), *id))
            .expect("prefix row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("prefix part")
        .expect("nonempty prefix part");
    journal
        .append(address.id, &part)
        .expect("append prefix part");
    let reader = Reader::open(directory.path()).expect("prefix reader");
    let expected: HashSet<_> = ids[..3].iter().map(|id| id.get()).collect();
    let active = one_segment(&reader);
    assert_eq!(
        active
            .dictionary_ids_with_prefix(prefix)
            .expect("active prefix"),
        expected
    );
    assert!(
        active
            .dictionary_ids_with_prefix(b"missing")
            .expect("unmatched prefix")
            .is_empty()
    );
    assert_eq!(
        active
            .dictionary_ids_with_prefix(b"")
            .expect("empty prefix")
            .len(),
        texts.len()
    );

    write_segment(&journal, &owner, address).expect("write prefix segment");
    let finished = one_segment(&reader);
    assert_eq!(finished.kind(), SegmentKind::Finished);
    assert_eq!(
        finished
            .dictionary_ids_with_prefix(prefix)
            .expect("finished prefix"),
        expected
    );
}
