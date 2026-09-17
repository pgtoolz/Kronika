use arrow_array::{Array as _, BinaryArray, BooleanArray, FixedSizeBinaryArray, UInt64Array};
use kronika_format::{DictLimits, Placement, crc32c};
use kronika_registry::{
    Bytes, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_DECODED_SECTION_BYTES, MAX_SECTION_BYTES,
    SECTION_WRITE_BATCH_ROWS, plain_parquet_decode_profile,
};
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::basic::Encoding;
use parquet::column::page::Page;
use parquet::file::reader::{FileReader, SerializedFileReader};
use sha2::{Digest as _, Sha256};

use super::{
    FINAL_DICT_WRITE_BATCH_ROWS, FINAL_DICT_WRITER_PROPS, encode, encode_final_entries,
    encode_final_entries_to, final_dictionary_body_bound,
};
use crate::Interner;

#[test]
fn write_compatible_limits_are_validated_at_configuration() {
    use super::validate_dict_limits_for_write;

    validate_dict_limits_for_write(DictLimits::new(4096, 64 * 1024).expect("collector limits"))
        .expect("collector limits fit the final body budget");
    validate_dict_limits_for_write(DictLimits::default())
        .expect("the default one MiB prefix fits one best-effort data page");
    let former_section_limit = DictLimits::new(4096, 8 * 1024 * 1024)
        .expect("dictionary limit fits the aggregate interner cap");
    validate_dict_limits_for_write(former_section_limit)
        .expect("the former eight MiB section limit is not a writer rejection boundary");
}

#[test]
fn final_dictionary_spans_bounded_plain_pages() {
    const ROWS: usize = FINAL_DICT_WRITE_BATCH_ROWS + 1;
    const VALUE_BYTES: usize = 1_100;

    assert_eq!(FINAL_DICT_WRITER_PROPS.write_batch_size(), 1024);
    assert_eq!(1_usize.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS), 1);
    assert_eq!(
        FINAL_DICT_WRITE_BATCH_ROWS.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS),
        1
    );
    assert_eq!(ROWS.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS), 2);
    assert_eq!(
        SECTION_WRITE_BATCH_ROWS.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS),
        64
    );

    let mut interner = Interner::new(DictLimits::new(4096, 4096).expect("limits"));
    for index in 0..ROWS {
        let mut value = vec![b'x'; VALUE_BYTES];
        value[..8].copy_from_slice(&u64::try_from(index).unwrap_or(u64::MAX).to_le_bytes());
        interner.intern(&value).expect("unique string interns");
    }
    let stored_bytes =
        usize::try_from(interner.stats().string_bytes).expect("test dictionary bytes fit usize");
    let bound = final_dictionary_body_bound(Placement::Strings, ROWS, stored_bytes, 0)
        .expect("multi-page dictionary is admitted");
    let [section] = encode_final_entries(interner.window().entries())
        .expect("write final dictionary")
        .try_into()
        .expect("only strings are present");
    assert!(section.body.len() <= bound);
    assert!(section.body.len() <= MAX_SECTION_BYTES);
    let profile = plain_parquet_decode_profile(&section.body, MAX_DECODED_SECTION_BYTES)
        .expect("multi-page dictionary passes shipped preflight");
    assert_eq!(profile.rows, ROWS);

    let reader = SerializedFileReader::new(Bytes::from(section.body))
        .expect("open final dictionary metadata");
    let row_group = reader.get_row_group(0).expect("row group");
    let bytes_column = row_group
        .metadata()
        .columns()
        .iter()
        .position(|column| column.column_path().string() == "bytes")
        .expect("bytes column");
    let mut pages = row_group
        .get_column_page_reader(bytes_column)
        .expect("bytes page reader");
    let mut data_pages = 0;
    while let Some(page) = pages.get_next_page().expect("read dictionary page") {
        if matches!(page, Page::DataPage { .. } | Page::DataPageV2 { .. }) {
            data_pages += 1;
        }
    }
    assert_eq!(data_pages, ROWS.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS));
}

#[test]
fn final_blob_dictionary_spans_pages_and_retains_truncation_metadata() {
    const ROWS: usize = FINAL_DICT_WRITE_BATCH_ROWS + 1;
    const VALUE_BYTES: usize = 1_100;
    const PREFIX_BYTES: usize = 4_096;

    let mut interner = Interner::new(DictLimits::new(1, PREFIX_BYTES).expect("limits"));
    for index in 0..ROWS {
        let mut value = vec![b'y'; VALUE_BYTES];
        value[..8].copy_from_slice(&u64::try_from(index).unwrap_or(u64::MAX).to_le_bytes());
        interner.intern(&value).expect("unique blob interns");
    }
    let long = vec![b'z'; PREFIX_BYTES + 1];
    let long_hash: [u8; 32] = Sha256::digest(&long).into();
    interner
        .intern(&long)
        .expect("long blob interns as a prefix");
    let stats = interner.stats();
    let stored_bytes = usize::try_from(stats.blob_bytes).expect("test bytes fit usize");
    let bound = final_dictionary_body_bound(
        Placement::Blobs,
        stats.blob_count,
        stored_bytes,
        stats.truncated_blob_count,
    )
    .expect("multi-page blob dictionary is admitted");
    let [section] = encode_final_entries(interner.window().entries())
        .expect("write final blob dictionary")
        .try_into()
        .expect("only blobs are present");
    assert_eq!(section.type_id, DICT_BLOBS_TYPE_ID);
    assert!(section.body.len() <= bound);
    let body = Bytes::from(section.body);
    let profile = plain_parquet_decode_profile(&body, MAX_DECODED_SECTION_BYTES)
        .expect("multi-page blob dictionary passes shipped preflight");
    assert_eq!(profile.rows, ROWS + 1);

    let metadata = SerializedFileReader::new(body.clone()).expect("open blob metadata");
    let row_group = metadata.get_row_group(0).expect("row group");
    let stored_column = row_group
        .metadata()
        .columns()
        .iter()
        .position(|column| column.column_path().string() == "stored_bytes")
        .expect("stored bytes column");
    let mut pages = row_group
        .get_column_page_reader(stored_column)
        .expect("stored bytes page reader");
    let mut data_pages = 0;
    while let Some(page) = pages.get_next_page().expect("read blob page") {
        if matches!(page, Page::DataPage { .. } | Page::DataPageV2 { .. }) {
            data_pages += 1;
        }
    }
    assert_eq!(data_pages, (ROWS + 1).div_ceil(FINAL_DICT_WRITE_BATCH_ROWS));

    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let batches = ParquetRecordBatchReaderBuilder::try_new_with_options(body, options)
        .expect("open blob rows")
        .build()
        .expect("build blob reader");
    let mut truncated_rows = 0;
    for batch in batches {
        let batch = batch.expect("read blob rows");
        let stored = batch
            .column_by_name("stored_bytes")
            .and_then(|column| column.as_any().downcast_ref::<BinaryArray>())
            .expect("stored bytes array");
        let full_len = batch
            .column_by_name("full_len")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
            .expect("full length array");
        let truncated = batch
            .column_by_name("truncated")
            .and_then(|column| column.as_any().downcast_ref::<BooleanArray>())
            .expect("truncated array");
        let hashes = batch
            .column_by_name("full_sha256")
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeBinaryArray>())
            .expect("hash array");
        for row in 0..batch.num_rows() {
            if truncated.value(row) {
                truncated_rows += 1;
                assert_eq!(stored.value(row), &long[..PREFIX_BYTES]);
                assert_eq!(
                    full_len.value(row),
                    u64::try_from(long.len()).unwrap_or(u64::MAX)
                );
                assert!(!hashes.is_null(row));
                assert_eq!(hashes.value(row), long_hash);
            } else {
                assert!(hashes.is_null(row));
                assert_eq!(
                    usize::try_from(full_len.value(row)).expect("blob length fits usize"),
                    stored.value(row).len()
                );
            }
        }
    }
    assert_eq!(truncated_rows, 1);
}

#[test]
fn an_empty_window_encodes_no_sections() {
    let interner = Interner::new(DictLimits::new(8, 1024).expect("limits"));
    assert!(encode(interner.window()).expect("encode").is_empty());
}

#[test]
fn strings_and_blobs_split_by_placement() {
    // blob_threshold 8: short values are strings, longer ones blobs.
    let mut interner = Interner::new(DictLimits::new(8, 1024).expect("limits"));
    interner.intern(b"short").expect("string");
    interner.intern(b"also").expect("string");
    interner
        .intern(b"a value longer than eight bytes")
        .expect("blob by size");

    let sections = encode(interner.window()).expect("encode");
    assert_eq!(sections.len(), 2, "one strings section, one blobs section");

    let strings = sections
        .iter()
        .find(|s| s.type_id == DICT_STRINGS_TYPE_ID)
        .expect("strings section");
    assert_eq!(strings.rows, 2);
    let blobs = sections
        .iter()
        .find(|s| s.type_id == DICT_BLOBS_TYPE_ID)
        .expect("blobs section");
    assert_eq!(blobs.rows, 1);
    assert_eq!(&blobs.body[..4], b"PAR1", "a Parquet body");
}

#[test]
fn bounded_dictionary_writer_preserves_exact_finished_bytes() {
    let mut interner = Interner::new(DictLimits::new(8, 16).expect("limits"));
    interner.intern(b"short").expect("string");
    interner
        .intern(b"a medium blob value")
        .expect("complete blob");
    interner
        .intern(b"a deliberately long blob value")
        .expect("truncated blob");
    let expected = encode_final_entries(interner.window().entries()).expect("legacy sections");

    let mut actual = Vec::new();
    let written = encode_final_entries_to(interner.window().entries(), &mut actual)
        .expect("bounded sections");
    assert_eq!(written.len(), expected.len());
    let mut offset = 0_usize;
    for (written, expected) in written.iter().zip(expected) {
        let len = usize::try_from(written.len).expect("test section length fits");
        let body = &actual[offset..offset + len];
        assert_eq!(written.type_id, expected.type_id);
        assert_eq!(written.rows, expected.rows);
        assert_eq!(written.crc32c, crc32c(body));
        assert_eq!(body, expected.body, "direct write preserves format bytes");
        offset += len;
    }
    assert_eq!(offset, actual.len());
}

#[test]
fn journal_dictionary_columns_have_no_dictionary_pages() {
    let mut interner = Interner::new(DictLimits::new(8, 4096).expect("limits"));
    for value in [
        b"short-a".as_slice(),
        b"short-b".as_slice(),
        b"a value longer than eight bytes".as_slice(),
        b"another value longer than eight bytes".as_slice(),
    ] {
        interner.intern(value).expect("dictionary value");
    }

    let sections = encode(interner.window()).expect("encode journal dictionary");
    assert_eq!(sections.len(), 2, "strings and blobs are both exercised");
    for section in sections {
        let reader = SerializedFileReader::new(Bytes::from(section.body))
            .expect("open journal dictionary metadata");
        for row_group in reader.metadata().row_groups() {
            for column in row_group.columns() {
                assert!(
                    column.dictionary_page_offset().is_none(),
                    "journal dictionary type {} column {} has a dictionary page",
                    section.type_id,
                    column.column_path().string()
                );
                assert!(
                    column.encodings().iter().all(|encoding| !matches!(
                        encoding,
                        Encoding::PLAIN_DICTIONARY | Encoding::RLE_DICTIONARY
                    )),
                    "journal dictionary type {} column {} advertises dictionary encoding",
                    section.type_id,
                    column.column_path().string()
                );
            }
        }
    }
}
