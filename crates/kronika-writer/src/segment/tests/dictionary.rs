//! Dictionary.

use arrow_array::{Array as _, BinaryArray, BooleanArray, FixedSizeBinaryArray, UInt64Array};

use super::*;

#[test]
fn a_finished_segment_carries_the_window_dictionary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let segment_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");

    // Intern two short strings and encode the window dictionary.
    let mut interner = Interner::new(DictLimits::new(4096, 1 << 20).expect("limits"));
    interner.intern(b"db-host-01").expect("intern");
    interner.intern(b"node-7").expect("intern");
    let dict_sections = dict::encode(interner.window()).expect("encode dictionary");

    // One data section plus the dictionary in a single part.
    let mut buffers = SectionBuffers::new();
    buffers.push(topology(1_000)).expect("buffer not full");
    let part = buffers
        .flush(&dict_sections)
        .expect("flush")
        .expect("a part");
    journal.append(address().id, &part).expect("append");

    let summary = write_segment(&journal, &owner, address()).expect("write the segment");
    assert_eq!(summary.sections, 2, "os_topology + dict.strings");

    let segment = std::fs::read(&segment_path).expect("read segment");
    let catalog = validate_part(&segment).expect("segment validates");
    let dict_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
        .expect("the dictionary section reached the segment");
    assert_eq!(dict_entry.rows, 2, "both interned strings");
    let start = usize::try_from(dict_entry.offset).unwrap();
    let end = start + usize::try_from(dict_entry.len).unwrap();
    assert_eq!(&segment[start..start + 4], b"PAR1", "a Parquet dict body");
    assert_eq!(&segment[end - 4..end], b"PAR1", "intact to its last byte");
}

#[test]
fn preserved_dictionary_accepts_duplicates_and_rejects_conflicts() {
    let bytes = b"preserved-value";
    let str_id = DictStrId::of(bytes).expect("nonzero string id");
    let mut dictionary = FinishedDictionary::default();
    dictionary
        .insert(str_id, Resolved::Str(bytes))
        .expect("insert string");
    dictionary
        .insert(str_id, Resolved::Str(bytes))
        .expect("identical duplicate");

    let conflicting = BlobEntry {
        str_id,
        stored_bytes: b"preserved",
        full_len: 99,
        truncated: true,
        full_sha256: Some([7; 32]),
    };
    assert!(matches!(
        dictionary.insert(str_id, Resolved::Blob(conflicting)),
        Err(WriteError::DictionaryConflict { str_id: raw }) if raw == str_id.get()
    ));
}

#[test]
fn preserved_dictionary_retains_blob_metadata_in_the_final_body() {
    let full = b"a blob whose retained prefix is shorter";
    let str_id = DictStrId::of(full).expect("nonzero blob id");
    let retained = &full[..12];
    let hash = [9; 32];
    let mut dictionary = FinishedDictionary::default();
    dictionary
        .insert(
            str_id,
            Resolved::Blob(BlobEntry {
                str_id,
                stored_bytes: retained,
                full_len: full.len() as u64,
                truncated: true,
                full_sha256: Some(hash),
            }),
        )
        .expect("insert truncated blob");

    let mut spool = Vec::new();
    let [section] = dictionary
        .write_sections_to(&mut spool, 41)
        .expect("write dictionary")
        .try_into()
        .expect("one blob section");
    assert_eq!(section.type_id(), DICT_BLOBS_TYPE_ID);
    assert_eq!(section.offset(), 41);
    assert_eq!(section.rows(), 1);
    assert_eq!(section.len(), spool.len() as u64);
    assert_eq!(section.crc32c(), crc32c(&spool));

    let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(spool))
        .expect("open blob dictionary")
        .build()
        .expect("build blob reader");
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .expect("read blob dictionary");
    let [batch] = batches.as_slice() else {
        panic!("one blob batch expected");
    };
    let stored = batch
        .column_by_name("stored_bytes")
        .and_then(|column| column.as_any().downcast_ref::<BinaryArray>())
        .expect("stored bytes");
    let full_len = batch
        .column_by_name("full_len")
        .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
        .expect("full length");
    let truncated = batch
        .column_by_name("truncated")
        .and_then(|column| column.as_any().downcast_ref::<BooleanArray>())
        .expect("truncated flag");
    let sha = batch
        .column_by_name("full_sha256")
        .and_then(|column| column.as_any().downcast_ref::<FixedSizeBinaryArray>())
        .expect("full hash");
    assert_eq!(stored.value(0), retained);
    assert_eq!(full_len.value(0), full.len() as u64);
    assert!(truncated.value(0));
    assert_eq!(sha.value(0), hash);
}
