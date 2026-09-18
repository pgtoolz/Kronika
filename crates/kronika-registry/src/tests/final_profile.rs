use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::{Field, Schema};
use bytes::Bytes;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::column::page::Page;
use parquet::file::reader::{FileReader, SerializedFileReader};

use super::{
    CodecError, FINAL_ZSTD_LEVEL, VerifiedSection, encode_final_batches, encode_final_sections_to,
    validate_final_section,
};
use crate::os_cpufreq::OsCpufreq;
use crate::os_loadavg::OsLoadavg;
use crate::{Section, StrId, Ts, decode_any};

fn row(load1: f64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(42),
        load1,
        load5: 2.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn decoded_batches(rows: &[OsLoadavg]) -> Vec<RecordBatch> {
    let body = OsLoadavg::encode(rows).expect("encode input section");
    decode_any(
        OsLoadavg::CONTRACT.type_id.get(),
        VerifiedSection::for_test(Bytes::from(body)),
    )
    .expect("decode input section")
    .batches
}

#[test]
fn finished_encoding_is_physical_and_boundary_deterministic() {
    let rows = [
        row(f64::from_bits(0x7ff8_0000_0000_0002)),
        row(-0.0),
        row(0.0),
        row(f64::from_bits(0x7ff8_0000_0000_0001)),
        row(-0.0),
    ];
    let one_batch = decoded_batches(&rows);
    let many_reversed = rows
        .iter()
        .rev()
        .flat_map(|row| decoded_batches(std::slice::from_ref(row)))
        .collect::<Vec<_>>();
    let type_id = OsLoadavg::CONTRACT.type_id.get();
    let one = encode_final_batches(type_id, one_batch).expect("write one batch");
    let many =
        encode_final_batches(type_id, many_reversed).expect("write reversed one-row batches");
    assert_eq!(one, many, "partition and input order must not affect bytes");

    let decoded = decode_any(type_id, VerifiedSection::for_test(Bytes::from(one.clone())))
        .expect("decode final section");
    assert_eq!(
        decoded.stats.rows,
        rows.len(),
        "duplicate rows are retained"
    );
    let typed = OsLoadavg::decode(VerifiedSection::for_test(Bytes::from(one.clone())))
        .expect("decode typed finished rows");
    assert_eq!(
        typed
            .iter()
            .map(|row| row.load1.to_bits())
            .collect::<Vec<_>>(),
        vec![
            (-0.0_f64).to_bits(),
            (-0.0_f64).to_bits(),
            0.0_f64.to_bits(),
            0x7ff8_0000_0000_0001,
            0x7ff8_0000_0000_0002,
        ],
        "canonical ordering preserves NaN payloads, signed zero, and duplicates"
    );
    assert_eq!(FINAL_ZSTD_LEVEL, 6);

    let reader = SerializedFileReader::new(Bytes::from(one)).expect("open Parquet metadata");
    let metadata = reader.metadata();
    assert_eq!(metadata.file_metadata().version(), 1);
    assert_eq!(metadata.file_metadata().created_by(), Some(""));
    assert_eq!(metadata.num_row_groups(), 1);
    let row_group = metadata.row_group(0);
    for column in row_group.columns() {
        assert!(matches!(column.compression(), Compression::ZSTD(_)));
        assert!(column.statistics().is_none());
        assert!(column.dictionary_page_offset().is_none());
        assert!(
            column
                .encodings()
                .iter()
                .all(|encoding| matches!(encoding, Encoding::PLAIN | Encoding::RLE)),
            "final columns use only PLAIN data and RLE levels"
        );
    }
    let group = reader.get_row_group(0).expect("row group");
    for column in 0..group.metadata().num_columns() {
        let mut pages = group.get_column_page_reader(column).expect("page reader");
        let mut data_pages = 0;
        while let Some(page) = pages.get_next_page().expect("read page") {
            if matches!(page, Page::DataPage { .. } | Page::DataPageV2 { .. }) {
                data_pages += 1;
            }
        }
        assert_eq!(data_pages, 1, "one data page per column chunk");
    }
}

#[test]
fn bounded_finalizer_preserves_exact_finished_bytes_with_ties() {
    let rows = [
        row(f64::from_bits(0x7ff8_0000_0000_0002)),
        row(-0.0),
        row(0.0),
        row(f64::from_bits(0x7ff8_0000_0000_0001)),
        row(-0.0),
    ];
    let type_id = OsLoadavg::CONTRACT.type_id.get();
    let bodies = rows
        .iter()
        .rev()
        .map(|row| Bytes::from(OsLoadavg::encode(std::slice::from_ref(row)).expect("encode row")))
        .collect::<Vec<_>>();
    let batches = bodies
        .iter()
        .flat_map(|body| {
            decode_any(type_id, VerifiedSection::for_test(body.clone()))
                .expect("decode row")
                .batches
        })
        .collect();
    let expected = encode_final_batches(type_id, batches).expect("write legacy final body");

    for body in &bodies {
        validate_final_section(type_id, VerifiedSection::for_test(body.clone()), 1)
            .expect("validate input section");
    }
    let mut actual = Vec::new();
    encode_final_sections_to(type_id, &vec![1; bodies.len()], &mut actual, |index| {
        Ok::<_, CodecError>(bodies[index].clone())
    })
    .expect("write bounded final body");

    assert_eq!(
        actual, expected,
        "bounded finishing must preserve format bytes"
    );
}

#[test]
fn bounded_finalizer_rejects_renamed_contract_column() {
    let batch = decoded_batches(&[row(1.0)])
        .pop()
        .expect("one encoded batch");
    let fields = batch
        .schema()
        .fields()
        .iter()
        .enumerate()
        .map(|(index, field)| {
            Field::new(
                if index == 0 { "wrong_ts" } else { field.name() },
                field.data_type().clone(),
                field.is_nullable(),
            )
        })
        .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new(fields));
    let renamed = RecordBatch::try_new(Arc::clone(&schema), batch.columns().to_vec())
        .expect("build renamed batch");
    let mut body = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut body, schema, None).expect("open Parquet writer");
    writer.write(&renamed).expect("write renamed batch");
    writer.close().expect("close renamed body");

    let error = encode_final_sections_to(
        OsLoadavg::CONTRACT.type_id.get(),
        &[1],
        &mut Vec::new(),
        |_index| Ok::<_, CodecError>(Bytes::from(body.clone())),
    )
    .expect_err("renamed contract column must be rejected");
    assert!(matches!(error, CodecError::SchemaMismatch));
}

#[test]
fn streamed_record_batches_preserve_finished_bytes_with_ties() {
    let rows = (0..8_193_i32)
        .map(|value| {
            let tied = value / 2;
            OsLoadavg {
                ts: Ts(i64::from(tied)),
                load1: f64::from(tied),
                load5: 2.0,
                load15: 0.5,
                running: 2,
                total: 345,
                scope: 0,
            }
        })
        .collect::<Vec<_>>();
    let expected_rows = rows
        .chunks(1_024)
        .map(|rows| u32::try_from(rows.len()).expect("test chunk length fits u32"))
        .collect::<Vec<_>>();
    let bodies = rows
        .chunks(1_024)
        .map(|rows| Bytes::from(OsLoadavg::encode(rows).expect("encode input section")))
        .collect::<Vec<_>>();
    let batches = bodies
        .iter()
        .flat_map(|body| {
            decode_any(
                OsLoadavg::CONTRACT.type_id.get(),
                VerifiedSection::for_test(body.clone()),
            )
            .expect("decode input section")
            .batches
        })
        .collect();
    let expected = encode_final_batches(OsLoadavg::CONTRACT.type_id.get(), batches)
        .expect("write reference body");
    let mut actual = Vec::new();
    encode_final_sections_to(
        OsLoadavg::CONTRACT.type_id.get(),
        &expected_rows,
        &mut actual,
        |index| Ok::<_, CodecError>(bodies[index].clone()),
    )
    .expect("write streamed final body");
    assert_eq!(actual, expected);
}

#[test]
fn reordered_finalizer_accepts_more_than_one_internal_write_batch() {
    const ROWS: usize = 65_537;
    const SPLIT: usize = 32_768;

    let make_rows = |range: std::ops::Range<usize>| {
        range
            .map(|value| OsLoadavg {
                ts: Ts(i64::try_from(value).expect("test timestamp fits i64")),
                load1: f64::from(u32::try_from(value).expect("test load fits u32")),
                load5: 2.0,
                load15: 0.5,
                running: 2,
                total: 345,
                scope: 0,
            })
            .collect::<Vec<_>>()
    };
    let high_rows = make_rows(SPLIT..ROWS);
    let low_rows = make_rows(0..SPLIT);
    let bodies = [
        Bytes::from(OsLoadavg::encode(&high_rows).expect("encode high timestamp range")),
        Bytes::from(OsLoadavg::encode(&low_rows).expect("encode low timestamp range")),
    ];
    let mut finished = Vec::new();
    encode_final_sections_to(
        OsLoadavg::CONTRACT.type_id.get(),
        &[
            u32::try_from(high_rows.len()).expect("high range row count fits u32"),
            u32::try_from(low_rows.len()).expect("low range row count fits u32"),
        ],
        &mut finished,
        |index| Ok::<_, CodecError>(bodies[index].clone()),
    )
    .expect("reorder more than one internal write batch");

    let decoded = OsLoadavg::decode(VerifiedSection::for_test(Bytes::from(finished)))
        .expect("decode reordered final section");
    assert_eq!(decoded.len(), ROWS);
    assert_eq!(decoded.first().map(|row| row.ts), Some(Ts(0)));
    assert_eq!(
        decoded.last().map(|row| row.ts),
        Some(Ts(i64::try_from(ROWS - 1).expect("test timestamp fits i64")))
    );
}

#[test]
fn cpufreq_sections_preserve_finished_bytes_and_bound_metadata() {
    let bodies = (0..360_i64)
        .map(|tick| {
            let rows = (0..128_i32)
                .map(|policy_id| OsCpufreq {
                    ts: Ts(tick * 1_000_000),
                    policy_id,
                    actual_source: Some(StrId(1)),
                    actual_frequency_hz: Some(2_200_000_000),
                    scaling_cur_freq_hz: Some(2_100_000_000),
                    scaling_min_freq_hz: Some(800_000_000),
                    scaling_max_freq_hz: Some(3_600_000_000),
                    online_cpus: Some(1),
                    scope: 0,
                })
                .collect::<Vec<_>>();
            Bytes::from(OsCpufreq::encode(&rows).expect("encode CPUFreq section"))
        })
        .collect::<Vec<_>>();
    let metadata_bytes = bodies
        .iter()
        .map(|body| {
            SerializedFileReader::new(body.clone())
                .expect("read CPUFreq metadata")
                .metadata()
                .memory_size()
        })
        .sum::<usize>();
    assert!(
        metadata_bytes > super::finalize::MAX_CACHED_METADATA_BYTES,
        "the profile must exercise cache-cap fallback"
    );

    let batches = bodies
        .iter()
        .flat_map(|body| {
            decode_any(
                OsCpufreq::CONTRACT.type_id.get(),
                VerifiedSection::for_test(body.clone()),
            )
            .expect("decode CPUFreq input")
            .batches
        })
        .collect();
    let expected = encode_final_batches(OsCpufreq::CONTRACT.type_id.get(), batches)
        .expect("write reference CPUFreq body");
    let mut actual = Vec::new();
    encode_final_sections_to(
        OsCpufreq::CONTRACT.type_id.get(),
        &vec![128; bodies.len()],
        &mut actual,
        |index| Ok::<_, CodecError>(bodies[index].clone()),
    )
    .expect("write bounded CPUFreq body");
    assert_eq!(actual, expected);
}
