use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::thrift::{TCompactOutputProtocol, TSerializable};
use thrift::protocol::{TInputProtocol, TOutputProtocol};

use crate::os_loadavg::OsLoadavg;
use crate::{CodecError, Section, Ts, VerifiedSection, decode_any};

use super::{
    BoundedCompactInput, MAGIC, MAX_THRIFT_NESTING, PageHeader, parquet_decode_profile,
    validate_parquet_decode_work,
};
use crate::MAX_DECODED_SECTION_BYTES;

fn row() -> OsLoadavg {
    OsLoadavg {
        ts: Ts(42),
        load1: 1.0,
        load5: 2.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn rewrite_first_page_header(mut body: Vec<u8>, edit: impl FnOnce(&mut PageHeader)) -> Vec<u8> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(body.clone()))
        .expect("read valid footer");
    let column = &builder.metadata().row_groups()[0].columns()[0];
    let start = usize::try_from(
        column
            .dictionary_page_offset()
            .unwrap_or_else(|| column.data_page_offset()),
    )
    .expect("positive column offset");
    let end = start + usize::try_from(column.compressed_size()).expect("positive compressed size");
    let mut input = BoundedCompactInput::new(&body[start..end]);
    let before = input.remaining_len();
    let mut header = PageHeader::read_from_in_protocol(&mut input).expect("read page header");
    let original_header_len = before - input.remaining_len();
    edit(&mut header);
    let mut encoded = Vec::new();
    {
        let mut output = TCompactOutputProtocol::new(&mut encoded);
        header
            .write_to_out_protocol(&mut output)
            .expect("encode changed header");
        output.flush().expect("flush header");
    }
    assert!(encoded.len() < end - start, "changed header fits chunk");
    let payload = body[start + original_header_len..end].to_vec();
    let keep = (end - start - encoded.len()).min(payload.len());
    encoded.extend_from_slice(&payload[..keep]);
    encoded.resize(end - start, 0);
    body[start..end].copy_from_slice(&encoded);
    body
}

#[test]
fn valid_body_is_admitted_at_its_exact_footer_work() {
    let body = OsLoadavg::encode(&[row()]).expect("encode section");
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(Bytes::from(body.clone())).expect("read footer");
    let work = builder
        .metadata()
        .row_groups()
        .iter()
        .flat_map(parquet::file::metadata::RowGroupMetaData::columns)
        .map(|column| usize::try_from(column.uncompressed_size()).expect("positive size"))
        .sum::<usize>();
    validate_parquet_decode_work(&body, work).expect("exact work is admitted");
    assert_eq!(
        parquet_decode_profile(&body, work).expect("profile"),
        super::ParquetDecodeProfile {
            rows: 1,
            decoded_bytes: work,
        }
    );
    assert!(matches!(
        validate_parquet_decode_work(&body, work - 1),
        Err(CodecError::DecodedSectionTooLarge { .. })
    ));
}

#[test]
fn oversized_page_header_is_rejected_before_parquet_decode() {
    let body = OsLoadavg::encode(&[row()]).expect("encode section");
    let body = rewrite_first_page_header(body, |header| {
        header.uncompressed_page_size =
            i32::try_from(MAX_DECODED_SECTION_BYTES + 1).expect("cap fits i32");
    });
    let err = decode_any(
        OsLoadavg::CONTRACT.type_id.get(),
        VerifiedSection::for_test(Bytes::from(body)),
    )
    .expect_err("oversized page claim is rejected");
    assert!(matches!(
        err,
        CodecError::Section { source, .. }
            if matches!(*source, CodecError::DecodedSectionTooLarge { .. })
    ));
}

#[test]
fn oversized_page_value_claim_is_rejected_before_decode() {
    let body = OsLoadavg::encode(&[row()]).expect("encode section");
    let body = rewrite_first_page_header(body, |header| {
        if let Some(dictionary) = header.dictionary_page_header.as_mut() {
            dictionary.num_values = i32::MAX;
        } else if let Some(data) = header.data_page_header.as_mut() {
            data.num_values = i32::MAX;
        } else if let Some(data) = header.data_page_header_v2.as_mut() {
            data.num_values = i32::MAX;
        } else {
            panic!("first page carries values");
        }
    });
    let err = decode_any(
        OsLoadavg::CONTRACT.type_id.get(),
        VerifiedSection::for_test(Bytes::from(body)),
    )
    .expect_err("oversized value claim is rejected");
    assert!(matches!(
        err,
        CodecError::Section { source, .. }
            if matches!(*source, CodecError::InvalidPageLayout)
    ));
}

fn plain_binary_body(encoding: parquet::basic::Encoding) -> Vec<u8> {
    use std::sync::Arc;

    let column: arrow_array::ArrayRef = Arc::new(arrow_array::BinaryArray::from(vec![
        b"alpha".as_slice(),
        b"beta".as_slice(),
    ]));
    let batch = arrow_array::RecordBatch::try_from_iter([("bytes", column)]).expect("build batch");
    let properties = parquet::file::properties::WriterProperties::builder()
        .set_dictionary_enabled(false)
        .set_encoding(encoding)
        .build();
    let mut body = Vec::new();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(&mut body, batch.schema(), Some(properties))
            .expect("create writer");
    writer.write(&batch).expect("write batch");
    writer.close().expect("close writer");
    body
}

#[test]
fn delta_byte_array_data_pages_are_rejected() {
    let body = plain_binary_body(parquet::basic::Encoding::PLAIN);
    super::validate_plain_parquet_decode_work(&body, MAX_DECODED_SECTION_BYTES)
        .expect("PLAIN data pages pass the profile");

    let body = plain_binary_body(parquet::basic::Encoding::DELTA_BYTE_ARRAY);
    assert!(matches!(
        super::validate_plain_parquet_decode_work(&body, MAX_DECODED_SECTION_BYTES),
        Err(CodecError::UnsupportedPageEncoding { .. })
    ));
}

#[test]
fn non_rle_level_encodings_are_rejected() {
    let body = plain_binary_body(parquet::basic::Encoding::PLAIN);
    let body = rewrite_first_page_header(body, |header| {
        let data = header
            .data_page_header
            .as_mut()
            .expect("dictionary is disabled, first page is data");
        data.definition_level_encoding = super::Encoding::DELTA_BINARY_PACKED;
    });
    assert!(matches!(
        super::validate_plain_parquet_decode_work(&body, MAX_DECODED_SECTION_BYTES),
        Err(CodecError::UnsupportedPageEncoding { .. })
    ));
}

#[test]
fn oversized_footer_collection_is_rejected_without_a_builder() {
    let metadata = [0x15, 0x02, 0x19, 0xfc, 0x81, 0x80, 0x04];
    let mut body = MAGIC.to_vec();
    body.extend_from_slice(&metadata);
    body.extend_from_slice(
        &u32::try_from(metadata.len())
            .expect("small handcrafted footer")
            .to_le_bytes(),
    );
    body.extend_from_slice(MAGIC);
    assert!(matches!(
        validate_parquet_decode_work(&body, MAX_DECODED_SECTION_BYTES),
        Err(CodecError::InvalidPageLayout)
    ));
}

#[test]
fn compact_reader_rejects_excessive_nesting() {
    let bytes = vec![0_u8; MAX_THRIFT_NESTING + 1];
    let mut input = BoundedCompactInput::new(&bytes);
    for _ in 0..MAX_THRIFT_NESTING {
        input.read_struct_begin().expect("within depth bound");
    }
    assert!(input.read_struct_begin().is_err());
}
