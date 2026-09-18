use super::*;

#[test]
fn current_dictionary_preserves_boundary_and_truncated_blob_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let active_address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let small = vec![0xff; DEFAULT_BLOB_THRESHOLD - 1];
    let boundary = vec![b'b'; DEFAULT_BLOB_THRESHOLD];
    let oversized = vec![b't'; DEFAULT_TRUNCATE_LIMIT + 1];
    let ignored = b"not selected".to_vec();
    let mut interner = Interner::new(DictLimits::default());
    let small_id = interner.intern(&small).expect("4095-byte string");
    let boundary_id = interner.intern(&boundary).expect("4096-byte blob");
    let oversized_id = interner.intern(&oversized).expect("truncated blob");
    let ignored_id = interner.intern(&ignored).expect("unrequested string");
    let dictionary = dict::encode(interner.window()).expect("collector dictionary output");
    let mut buffers = SectionBuffers::new();
    for (cpu_id, id) in [small_id, boundary_id, oversized_id, ignored_id]
        .into_iter()
        .enumerate()
    {
        let cpu_id = i32::try_from(cpu_id).expect("four fixture rows fit i32");
        buffers
            .push(topology(100 + i64::from(cpu_id), cpu_id, id))
            .expect("buffer topology row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode current collector part")
        .expect("part has rows");
    journal
        .append(active_address.id, &part)
        .expect("append current collector output");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let dictionary = segment.dictionary().expect("decode complete dictionary");
    assert_eq!(dictionary.entries().count(), 4);
    assert_eq!(
        dictionary.resolve(small_id.get()),
        Some(Resolved::Str(&small))
    );
    assert_eq!(
        dictionary.resolve(boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
    let expected_hash: [u8; 32] = Sha256::digest(&oversized).into();
    assert_eq!(
        dictionary.resolve(oversized_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: oversized_id,
            stored_bytes: &oversized[..DEFAULT_TRUNCATE_LIMIT],
            full_len: oversized.len() as u64,
            truncated: true,
            full_sha256: Some(expected_hash),
        }))
    );
    let selected = segment
        .dictionary_for(&HashSet::from([
            small_id.get(),
            boundary_id.get(),
            oversized_id.get(),
        ]))
        .expect("decode selected dictionary values");
    assert_eq!(
        selected.resolve(small_id.get()),
        Some(Resolved::Str(&small))
    );
    assert_eq!(
        selected.resolve(boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
    assert_eq!(
        selected.resolve(oversized_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: oversized_id,
            stored_bytes: &oversized[..DEFAULT_TRUNCATE_LIMIT],
            full_len: oversized.len() as u64,
            truncated: true,
            full_sha256: Some(expected_hash),
        }))
    );
    assert_eq!(selected.resolve(ignored_id.get()), None);
    assert_model_names_resolve(&segment, &dictionary);
}

#[test]
fn finished_dictionary_preserves_boundary_blob_metadata() {
    let directory = tempfile::tempdir().expect("finished tempdir");
    let owner = writer(&directory);
    let finished_address = address(SEGMENT_ID + 1);
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open finished journal");
    let boundary = vec![b'b'; DEFAULT_BLOB_THRESHOLD];
    let boundary_id = append_text_window(&mut journal, finished_address.id, 200, &boundary);
    write_segment(&journal, &owner, finished_address).expect("publish finished blob output");
    let reader = Reader::open(directory.path()).expect("open finished reader");
    let finished = one_segment(&reader);
    assert_eq!(finished.kind(), SegmentKind::Finished);
    let dictionary = finished.dictionary().expect("finished dictionary");
    assert_eq!(
        dictionary.resolve(boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
}
