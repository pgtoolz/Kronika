//! Shared code for Parquet section codecs.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, LazyLock};

use arrow_array::builder::{Int32Builder, ListBuilder};
use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, ArrowPrimitiveType, BooleanArray, ListArray, PrimitiveArray, RecordBatch,
    RecordBatchReader,
};
use arrow_ord::sort::{SortColumn, lexsort_to_indices};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use arrow_select::{concat::concat_batches, take::take};
use bytes::Bytes;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::{
    ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};
use parquet::arrow::arrow_writer::ArrowWriterOptions;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

use crate::contract::{ColumnType, TypeContract};

pub mod instance_metadata;
pub mod os_block_topology;
pub mod os_cgroup_context;
pub mod os_cgroup_cpu;
pub mod os_cgroup_io;
pub mod os_cgroup_mapping;
pub mod os_cgroup_memory;
pub mod os_cgroup_pids;
pub mod os_cpu;
pub mod os_cpufreq;
pub mod os_diskstats;
pub mod os_interrupts;
pub mod os_kernel_limits;
pub mod os_loadavg;
pub mod os_meminfo;
pub mod os_mountinfo;
pub mod os_netdev;
pub mod os_netstat;
pub mod os_nfs;
pub mod os_numa;
pub mod os_process;
pub mod os_process_status;
pub mod os_psi;
pub mod os_snmp;
pub mod os_snmp6;
pub mod os_softirq;
pub mod os_stat;
pub mod os_topology;
pub mod os_user;
pub mod os_vmstat;
pub mod pg_locks;
pub mod pg_log;
pub mod pg_prepared_xacts;
pub mod pg_settings;
pub mod pg_stat_activity;
pub mod pg_stat_archiver;
pub mod pg_stat_bgwriter;
pub mod pg_stat_checkpointer;
pub mod pg_stat_database;
pub mod pg_stat_io;
pub mod pg_stat_progress_vacuum;
pub mod pg_stat_statements;
pub mod pg_stat_statements_info;
pub mod pg_stat_user_indexes;
pub mod pg_stat_user_tables;
pub mod pg_stat_wal;
pub mod pg_store_plans;
pub mod pg_store_plans_info;
pub mod pg_wal_storage;
pub mod pgbouncer_events;

mod bounds;
mod columns;
mod decode;
mod encode;
mod error;
mod finalize;

pub use bounds::{
    FinalPlainColumnSize, final_data_body_bound, final_single_batch_plain_body_bound,
};
pub use columns::{
    ListColumn, arrow_schema, nullable_bool, nullable_column, opt_bool, opt_primitive,
    read_list_i32, required_bool, required_column, write_bool, write_bool_nullable, write_list_i32,
    write_nullable, write_required,
};
pub(crate) use columns::{schema_matches, validate_list_i32_batch};
use decode::{boolean_column, capped_reader};
pub use encode::encode_final_batches;
use encode::sort_by_sort_key;
pub use error::CodecError;
pub use finalize::{encode_final_sections_to, validate_final_section};

/// Maximum rows addressable by the version-1 catalog field.
pub const MAX_SECTION_ROWS: usize = u32::MAX as usize;

/// Rows retained in one internal writer batch or final Parquet row group.
pub const SECTION_WRITE_BATCH_ROWS: usize = 65_536;

/// Maximum accepted section byte length under the version-1 container envelope.
///
/// Checked before Parquet metadata is parsed.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the one GiB format envelope fits every supported usize"
)]
pub const MAX_SECTION_BYTES: usize = kronika_format::MAX_PHYSICAL_SECTION_BYTES as usize;

/// Maximum aggregate uncompressed Parquet column bytes admitted before decode.
pub const MAX_DECODED_SECTION_BYTES: usize = MAX_SECTION_BYTES;

/// Maximum Parquet row groups accepted in one snapshot section.
///
/// Decode rejects excessive row groups before reading column data.
pub const MAX_ROW_GROUPS: usize = MAX_SECTION_ROWS.div_ceil(SECTION_WRITE_BATCH_ROWS);

/// Maximum `List<Int32>` child values accepted in one row.
pub(crate) const MAX_LIST_I32_VALUES_PER_ROW: usize = 4096;

/// Maximum `List<Int32>` child values accepted in one section.
pub(crate) const MAX_LIST_I32_VALUES_PER_SECTION: usize =
    MAX_DECODED_SECTION_BYTES / size_of::<i32>();

// ---- Encode shared code ----------------------------------------------------

pub(crate) fn row_count_fits(rows: usize) -> bool {
    u32::try_from(rows).is_ok()
}

/// Reject a row count above [`MAX_SECTION_ROWS`] before columns are built.
pub(crate) fn check_row_cap(rows: usize) -> Result<(), CodecError> {
    if !row_count_fits(rows) {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        });
    }
    Ok(())
}

/// Initial capacity for a small snapshot section.
const ENCODE_BUF_HINT: usize = 4096;

/// Parquet writer properties shared by every snapshot section.
static WRITER_PROPS: LazyLock<WriterProperties> = LazyLock::new(|| {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(3).expect("zstd level 3 is valid"),
        ))
        .set_max_row_group_size(SECTION_WRITE_BATCH_ROWS)
        .set_created_by(String::new())
        .build()
});

/// Zstandard level used for coalesced sections in a finished segment.
pub const FINAL_ZSTD_LEVEL: i32 = 6;

/// Target data-page size for coalesced final sections.
pub const FINAL_DATA_PAGE_BYTES: usize = 1024 * 1024;

/// Fixed allowance for one page header and its column-chunk metadata.
const FINAL_PAGE_FRAMING_BOUND: usize = 4 * 1024;

/// Fixed allowance for the Parquet header, schema, row-group and file footer.
const FINAL_FILE_FRAMING_BOUND: usize = 64 * 1024;

/// Parquet properties for the single final body of each populated type.
static FINAL_WRITER_PROPS: LazyLock<WriterProperties> = LazyLock::new(|| {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(FINAL_ZSTD_LEVEL).expect("zstd level 6 is valid"),
        ))
        .set_max_row_group_size(SECTION_WRITE_BATCH_ROWS)
        .set_data_page_size_limit(FINAL_DATA_PAGE_BYTES)
        .set_data_page_row_count_limit(SECTION_WRITE_BATCH_ROWS)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::None)
        .set_offset_index_disabled(true)
        .set_created_by(String::new())
        .build()
});

/// Encode pre-built columns into a Parquet section body.
pub(crate) fn encode_section(
    contract: &TypeContract,
    columns: Vec<ArrayRef>,
) -> Result<Vec<u8>, CodecError> {
    let schema = arrow_schema(contract);
    let batch = RecordBatch::try_new(Arc::clone(&schema), columns)?;
    check_row_cap(batch.num_rows())?;
    let batch = sort_by_sort_key(&batch, contract)?;

    let options = ArrowWriterOptions::new()
        .with_properties(WRITER_PROPS.clone())
        .with_skip_arrow_metadata(true);

    let mut buf = Vec::with_capacity(ENCODE_BUF_HINT);
    let mut writer = ArrowWriter::try_new_with_options(&mut buf, schema, options)?;
    writer.write(&batch)?;
    writer.close()?;

    if buf.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: buf.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    Ok(buf)
}

// ---- Decode shared code ----------------------------------------------------

/// Section bytes whose CRC has been checked against the catalog.
///
/// Decode entry points take this instead of raw `Bytes`.
#[derive(Clone, Debug)]
pub struct VerifiedSection(Bytes);

impl VerifiedSection {
    /// Verify `bytes` against their catalog CRC and wrap them for decode.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::SectionCrcMismatch`] when the CRC differs.
    pub fn verify(
        bytes: Bytes,
        expected: u32,
        crc32c: impl FnOnce(&[u8]) -> u32,
    ) -> Result<Self, CodecError> {
        let got = crc32c(&bytes);
        if got == expected {
            Ok(Self(bytes))
        } else {
            Err(CodecError::SectionCrcMismatch { expected, got })
        }
    }

    /// Wrap bytes without a CRC check, for tests that decode their own output.
    #[cfg(test)]
    pub(crate) const fn for_test(bytes: Bytes) -> Self {
        Self(bytes)
    }

    /// Unwrap the verified bytes.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// The section byte length.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the section is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod verified_section_tests;

#[cfg(test)]
mod final_profile_tests;

#[cfg(test)]
mod bounds_tests;

#[cfg(test)]
mod codec_error_tests;

/// What a section decode processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeStats {
    /// The decoded section's `type_id`.
    pub type_id: u32,
    /// Input section bytes.
    pub bytes_in: usize,
    /// Parquet row groups read.
    pub row_groups: usize,
    /// Arrow batches produced.
    pub batches: usize,
    /// Rows decoded.
    pub rows: usize,
    /// Child values decoded across every `ListI32` column.
    pub list_i32_child_values: usize,
}

/// A decoded section: its Arrow batches and the [`DecodeStats`] for the call.
#[derive(Debug)]
pub struct DecodedSection {
    /// The section's rows, in contract column order.
    pub batches: Vec<RecordBatch>,
    /// What the decode processed.
    pub stats: DecodeStats,
}

/// Parquet read batch size: the reader yields batches of at most this many rows.
pub(crate) const DECODE_BATCH_SIZE: usize = 8192;

/// Decode a Parquet section body, streaming batches into `push_rows`.
pub(crate) fn decode_section<Row>(
    contract: &TypeContract,
    section: VerifiedSection,
    mut push_rows: impl FnMut(&RecordBatch, &mut Vec<Row>) -> Result<(), CodecError>,
) -> Result<Vec<Row>, CodecError> {
    let (reader, _row_groups, _claimed_rows) = capped_reader(section.into_bytes())?;
    if !schema_matches(&reader.schema(), contract) {
        return Err(CodecError::SchemaMismatch);
    }
    let list_columns: Vec<&'static str> = contract
        .columns
        .iter()
        .filter(|column| column.ty == ColumnType::ListI32)
        .map(|column| column.name)
        .collect();
    let mut list_child_values = vec![0_usize; list_columns.len()];
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let next_rows =
            rows.len()
                .checked_add(batch.num_rows())
                .ok_or(CodecError::TooManyRows {
                    rows: usize::MAX,
                    max: MAX_SECTION_ROWS,
                })?;
        if !row_count_fits(next_rows) {
            return Err(CodecError::TooManyRows {
                rows: next_rows,
                max: MAX_SECTION_ROWS,
            });
        }
        for (i, &name) in list_columns.iter().enumerate() {
            let values = validate_list_i32_batch(&batch, name)?;
            list_child_values[i] =
                list_child_values[i]
                    .checked_add(values)
                    .ok_or(CodecError::TooManyListValues {
                        name,
                        values: usize::MAX,
                        max: MAX_LIST_I32_VALUES_PER_SECTION,
                    })?;
            if list_child_values[i] > MAX_LIST_I32_VALUES_PER_SECTION {
                return Err(CodecError::TooManyListValues {
                    name,
                    values: list_child_values[i],
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                });
            }
        }
        push_rows(&batch, &mut rows)?;
    }
    Ok(rows)
}

/// Decode a section body to Arrow batches.
pub(crate) fn decode_batches(
    contract: &TypeContract,
    section: VerifiedSection,
) -> Result<DecodedSection, CodecError> {
    let bytes = section.into_bytes();
    let bytes_in = bytes.len();
    let (reader, row_groups, _claimed_rows) = capped_reader(bytes)?;

    if !schema_matches(&reader.schema(), contract) {
        return Err(CodecError::SchemaMismatch);
    }

    let list_columns: Vec<&'static str> = contract
        .columns
        .iter()
        .filter(|column| column.ty == ColumnType::ListI32)
        .map(|column| column.name)
        .collect();
    let mut list_child_values = vec![0_usize; list_columns.len()];
    let mut batches = Vec::new();
    let mut rows = 0_usize;
    for batch in reader {
        let batch = batch?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        if !row_count_fits(rows) {
            return Err(CodecError::TooManyRows {
                rows,
                max: MAX_SECTION_ROWS,
            });
        }
        for (i, &name) in list_columns.iter().enumerate() {
            let values = validate_list_i32_batch(&batch, name)?;
            list_child_values[i] =
                list_child_values[i]
                    .checked_add(values)
                    .ok_or(CodecError::TooManyListValues {
                        name,
                        values: usize::MAX,
                        max: MAX_LIST_I32_VALUES_PER_SECTION,
                    })?;
            if list_child_values[i] > MAX_LIST_I32_VALUES_PER_SECTION {
                return Err(CodecError::TooManyListValues {
                    name,
                    values: list_child_values[i],
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                });
            }
        }
        batches.push(batch);
    }
    let list_i32_child_values = list_child_values
        .iter()
        .try_fold(0_usize, |total, &values| {
            total
                .checked_add(values)
                .ok_or(CodecError::TooManyListValues {
                    name: list_columns.first().copied().unwrap_or("ListI32"),
                    values: usize::MAX,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                })
        })?;
    let stats = DecodeStats {
        type_id: contract.type_id.get(),
        bytes_in,
        row_groups,
        batches: batches.len(),
        rows,
        list_i32_child_values,
    };
    Ok(DecodedSection { batches, stats })
}

#[cfg(test)]
mod list_i32_tests;

#[cfg(test)]
mod hygiene_tests;
