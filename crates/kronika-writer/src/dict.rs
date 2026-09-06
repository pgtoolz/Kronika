//! `dict.strings` / `dict.blobs` section encoders.
//!
//! Encode `dict.strings` and `dict.blobs` section bodies.

use std::io::{self, Write};
use std::ops::Range;
use std::sync::{Arc, LazyLock};

use arrow_array::{
    ArrayRef, BinaryArray, BooleanArray, FixedSizeBinaryArray, RecordBatch, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use kronika_format::{Crc32c, DictLimits, EntrySnapshot, Placement, SegmentDicts};
use kronika_registry::{
    CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, FinalPlainColumnSize, MAX_SECTION_BYTES,
    MAX_SECTION_ROWS, SECTION_WRITE_BATCH_ROWS, final_single_batch_plain_body_bound,
};
use parquet::arrow::arrow_writer::{
    ArrowColumnWriter, ArrowWriterOptions, compute_leaves, get_column_writers,
};
use parquet::arrow::{ArrowSchemaConverter, ArrowWriter};
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};

/// Rows handed to each final dictionary column writer at once.
const FINAL_DICT_WRITE_BATCH_ROWS: usize = 1024;

/// Parquet writer properties shared by dictionary sections.
static DICT_WRITER_PROPS: LazyLock<WriterProperties> = LazyLock::new(|| {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(3).expect("zstd level 3 is valid"),
        ))
        .set_max_row_group_size(SECTION_WRITE_BATCH_ROWS)
        .set_dictionary_enabled(false)
        .set_created_by(String::new())
        .build()
});

/// Final finished-dictionary properties. Journal windows retain their cheap
/// append profile; normalization uses this profile exactly once at write.
static FINAL_DICT_WRITER_PROPS: LazyLock<WriterProperties> = LazyLock::new(|| {
    WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(kronika_registry::FINAL_ZSTD_LEVEL).expect("zstd level 6 is valid"),
        ))
        .set_max_row_group_size(SECTION_WRITE_BATCH_ROWS)
        .set_data_page_size_limit(1024 * 1024)
        .set_data_page_row_count_limit(SECTION_WRITE_BATCH_ROWS)
        .set_write_batch_size(FINAL_DICT_WRITE_BATCH_ROWS)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::None)
        .set_offset_index_disabled(true)
        .set_created_by(String::new())
        .build()
});

/// One encoded dictionary section: its type id, row count, and Parquet body.
#[derive(Debug, Clone)]
pub struct DictSection {
    /// [`DICT_STRINGS_TYPE_ID`] or [`DICT_BLOBS_TYPE_ID`].
    pub type_id: u32,
    /// Number of dictionary entries in the body.
    pub rows: u32,
    /// The Parquet section body.
    pub body: Vec<u8>,
}

/// One dictionary section written directly to a finished-segment spool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WrittenDictSection {
    pub(crate) type_id: u32,
    pub(crate) rows: u32,
    pub(crate) len: u64,
    pub(crate) crc32c: u32,
}

/// Encode a dictionary window into section bodies.
///
/// # Errors
///
/// Returns [`CodecError`] when row caps or Parquet encoding fail.
pub fn encode(window: &SegmentDicts) -> Result<Vec<DictSection>, CodecError> {
    encode_entries_with_properties(window.entries(), &DICT_WRITER_PROPS)
}

/// Encode one normalized segment dictionary with the finished segment profile.
#[cfg(test)]
#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
pub(crate) fn encode_final_entries<'a>(
    entries: impl IntoIterator<Item = EntrySnapshot<'a>>,
) -> Result<Vec<DictSection>, CodecError> {
    let entries = entries.into_iter().collect::<Vec<_>>();
    validate_final_entries(&entries)?;
    encode_entries_with_properties(entries, &FINAL_DICT_WRITER_PROPS)
}

/// Encode normalized dictionary entries directly to a bounded output spool.
///
/// Only one output column and at most [`FINAL_DICT_WRITE_BATCH_ROWS`] values
/// are materialized at once. Returned metadata describes consecutive bodies
/// in `out` in their write order.
#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
pub(crate) fn encode_final_entries_to<'a>(
    entries: impl IntoIterator<Item = EntrySnapshot<'a>>,
    out: &mut (impl Write + Send),
) -> Result<Vec<WrittenDictSection>, CodecError> {
    let entries = entries.into_iter().collect::<Vec<_>>();
    validate_final_entries(&entries)?;
    let (mut strings, mut blobs) = partition_entries(entries);
    let mut written = Vec::with_capacity(2);
    if !strings.is_empty() {
        strings.sort_unstable_by_key(|entry| entry.str_id.get());
        written.push(write_final_dictionary(DICT_STRINGS_TYPE_ID, &strings, out)?);
    }
    if !blobs.is_empty() {
        blobs.sort_unstable_by_key(|entry| entry.str_id.get());
        written.push(write_final_dictionary(DICT_BLOBS_TYPE_ID, &blobs, out)?);
    }
    Ok(written)
}

fn validate_final_entries(entries: &[EntrySnapshot<'_>]) -> Result<(), CodecError> {
    let mut string_rows = 0_usize;
    let mut string_bytes = 0_usize;
    let mut blob_rows = 0_usize;
    let mut blob_bytes = 0_usize;
    let mut truncated_blobs = 0_usize;
    for entry in entries {
        match entry.placement {
            Placement::Strings => {
                string_rows += 1;
                string_bytes = string_bytes.checked_add(entry.stored_bytes.len()).ok_or(
                    CodecError::SectionTooLarge {
                        len: usize::MAX,
                        max: MAX_SECTION_BYTES,
                    },
                )?;
            }
            Placement::Blobs => {
                blob_rows += 1;
                blob_bytes = blob_bytes.checked_add(entry.stored_bytes.len()).ok_or(
                    CodecError::SectionTooLarge {
                        len: usize::MAX,
                        max: MAX_SECTION_BYTES,
                    },
                )?;
                truncated_blobs += usize::from(entry.truncated);
            }
        }
    }
    if string_rows != 0 {
        final_dictionary_body_bound(Placement::Strings, string_rows, string_bytes, 0)?;
    }
    if blob_rows != 0 {
        final_dictionary_body_bound(Placement::Blobs, blob_rows, blob_bytes, truncated_blobs)?;
    }
    Ok(())
}

fn partition_entries(
    entries: Vec<EntrySnapshot<'_>>,
) -> (Vec<EntrySnapshot<'_>>, Vec<EntrySnapshot<'_>>) {
    entries
        .into_iter()
        .partition(|entry| entry.placement == Placement::Strings)
}

/// Proves that every value the limits admit can also be finished.
///
/// # Errors
///
/// Returns [`CodecError::SectionTooLarge`] when `truncate_limit` admits a
/// single stored value above the finished dictionary body cap.
pub fn validate_dict_limits_for_write(limits: DictLimits) -> Result<(), CodecError> {
    final_dictionary_body_bound(Placement::Strings, 1, limits.truncate_limit(), 0)?;
    final_dictionary_body_bound(Placement::Blobs, 1, limits.truncate_limit(), 1).map(drop)
}

/// Check one normalized dictionary body's PLAIN page and encoded-size bounds.
///
/// # Errors
///
/// Returns [`CodecError`] when row arithmetic overflows or the conservative
/// multi-page body bound crosses the version-1 section envelope.
pub fn final_dictionary_body_bound(
    placement: Placement,
    rows: usize,
    stored_bytes: usize,
    truncated_rows: usize,
) -> Result<usize, CodecError> {
    check_dict_rows(rows)?;
    if truncated_rows > rows || (placement == Placement::Strings && truncated_rows != 0) {
        return Err(CodecError::SchemaMismatch);
    }
    let too_large = || CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    };
    let ids = rows.checked_mul(8).ok_or_else(&too_large)?;
    let binary = rows
        .checked_mul(4)
        .and_then(|offsets| offsets.checked_add(stored_bytes))
        .ok_or_else(&too_large)?;
    let mut columns = vec![
        FinalPlainColumnSize::new(ids, 0),
        FinalPlainColumnSize::new(binary, 0),
    ];
    if placement == Placement::Blobs {
        let full_len = rows.checked_mul(8).ok_or_else(&too_large)?;
        let sha = truncated_rows.checked_mul(32).ok_or_else(&too_large)?;
        let sha_levels = rows
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(8))
            .ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?;
        columns.extend([
            FinalPlainColumnSize::new(full_len, 0),
            FinalPlainColumnSize::new(rows, 0),
            FinalPlainColumnSize::new(sha, sha_levels),
        ]);
    }
    let pages_per_column = rows.div_ceil(FINAL_DICT_WRITE_BATCH_ROWS).max(1);
    final_single_batch_plain_body_bound(columns, pages_per_column)
}

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
fn encode_entries_with_properties<'a>(
    entries: impl IntoIterator<Item = EntrySnapshot<'a>>,
    properties: &WriterProperties,
) -> Result<Vec<DictSection>, CodecError> {
    let mut strings: Vec<EntrySnapshot<'_>> = Vec::new();
    let mut blobs: Vec<EntrySnapshot<'_>> = Vec::new();
    for entry in entries {
        match entry.placement {
            Placement::Strings => strings.push(entry),
            Placement::Blobs => blobs.push(entry),
        }
    }

    let mut sections = Vec::new();
    if !strings.is_empty() {
        sections.push(encode_strings(&mut strings, properties)?);
    }
    if !blobs.is_empty() {
        sections.push(encode_blobs(&mut blobs, properties)?);
    }
    Ok(sections)
}

/// Reject an over-cap dictionary section before Arrow arrays are built.
const fn check_dict_rows(rows: usize) -> Result<(), CodecError> {
    if rows > MAX_SECTION_ROWS {
        Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        })
    } else {
        Ok(())
    }
}

/// `dict.strings`: `str_id u64, bytes binary`, sorted by `str_id`.
fn encode_strings(
    entries: &mut [EntrySnapshot<'_>],
    properties: &WriterProperties,
) -> Result<DictSection, CodecError> {
    check_dict_rows(entries.len())?;
    entries.sort_unstable_by_key(|entry| entry.str_id.get());
    let ids = UInt64Array::from_iter_values(entries.iter().map(|entry| entry.str_id.get()));
    let bytes = BinaryArray::from_iter_values(entries.iter().map(|entry| entry.stored_bytes));
    let schema = Arc::new(Schema::new(vec![
        Field::new("str_id", DataType::UInt64, false),
        Field::new("bytes", DataType::Binary, false),
    ]));
    let batch = RecordBatch::try_new(schema, vec![Arc::new(ids), Arc::new(bytes)])?;
    section(DICT_STRINGS_TYPE_ID, &batch, properties)
}

/// `dict.blobs`: `str_id`, `stored_bytes`, `full_len`, `truncated`, and the
/// optional `full_sha256` present only for truncated values.
fn encode_blobs(
    entries: &mut [EntrySnapshot<'_>],
    properties: &WriterProperties,
) -> Result<DictSection, CodecError> {
    check_dict_rows(entries.len())?;
    entries.sort_unstable_by_key(|entry| entry.str_id.get());
    let ids = UInt64Array::from_iter_values(entries.iter().map(|entry| entry.str_id.get()));
    let stored = BinaryArray::from_iter_values(entries.iter().map(|entry| entry.stored_bytes));
    let full_len = UInt64Array::from_iter_values(entries.iter().map(|entry| entry.full_len));
    let truncated: BooleanArray = entries.iter().map(|entry| Some(entry.truncated)).collect();
    let sha = FixedSizeBinaryArray::try_from_sparse_iter_with_size(
        entries.iter().map(|entry| entry.full_sha256),
        32,
    )?;

    let schema = Arc::new(Schema::new(vec![
        Field::new("str_id", DataType::UInt64, false),
        Field::new("stored_bytes", DataType::Binary, false),
        Field::new("full_len", DataType::UInt64, false),
        Field::new("truncated", DataType::Boolean, false),
        Field::new("full_sha256", DataType::FixedSizeBinary(32), true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(ids),
        Arc::new(stored),
        Arc::new(full_len),
        Arc::new(truncated),
        Arc::new(sha),
    ];
    let batch = RecordBatch::try_new(schema, columns)?;
    section(DICT_BLOBS_TYPE_ID, &batch, properties)
}

fn write_final_dictionary(
    type_id: u32,
    entries: &[EntrySnapshot<'_>],
    out: &mut (impl Write + Send),
) -> Result<WrittenDictSection, CodecError> {
    check_dict_rows(entries.len())?;
    let schema = dictionary_schema(type_id)?;
    let properties = Arc::new(FINAL_DICT_WRITER_PROPS.clone());
    let parquet_schema = ArrowSchemaConverter::new()
        .with_coerce_types(properties.coerce_types())
        .convert(&schema)?;
    let column_writers = get_column_writers(&parquet_schema, &properties, &schema)?;
    let mut sink = DictSink::new(out);
    let mut file = SerializedFileWriter::new(
        &mut sink,
        parquet_schema.root_schema_ptr(),
        Arc::clone(&properties),
    )?;
    let mut row_group = file.next_row_group()?;
    let mut column_writers = column_writers.into_iter();
    for (column, field) in schema.fields().iter().enumerate() {
        write_dictionary_column(
            type_id,
            column,
            field,
            entries,
            &mut column_writers,
            &mut row_group,
        )?;
    }
    if column_writers.next().is_some() {
        return Err(CodecError::SchemaMismatch);
    }
    row_group.close()?;
    file.close()?;
    let (len, crc32c) = sink.finish();
    let len_usize = usize::try_from(len).map_err(|_overflow| CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    })?;
    if len_usize == 0 || len_usize > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: len_usize,
            max: MAX_SECTION_BYTES,
        });
    }
    Ok(WrittenDictSection {
        type_id,
        rows: u32::try_from(entries.len()).unwrap_or(u32::MAX),
        len,
        crc32c,
    })
}

fn dictionary_schema(type_id: u32) -> Result<Arc<Schema>, CodecError> {
    let fields = match type_id {
        DICT_STRINGS_TYPE_ID => vec![
            Field::new("str_id", DataType::UInt64, false),
            Field::new("bytes", DataType::Binary, false),
        ],
        DICT_BLOBS_TYPE_ID => vec![
            Field::new("str_id", DataType::UInt64, false),
            Field::new("stored_bytes", DataType::Binary, false),
            Field::new("full_len", DataType::UInt64, false),
            Field::new("truncated", DataType::Boolean, false),
            Field::new("full_sha256", DataType::FixedSizeBinary(32), true),
        ],
        _ => return Err(CodecError::UnknownType { type_id }),
    };
    Ok(Arc::new(Schema::new(fields)))
}

fn write_dictionary_column<W: Write + Send>(
    type_id: u32,
    column: usize,
    field: &Arc<Field>,
    entries: &[EntrySnapshot<'_>],
    column_writers: &mut impl Iterator<Item = ArrowColumnWriter>,
    row_group: &mut SerializedRowGroupWriter<'_, W>,
) -> Result<(), CodecError> {
    let mut parquet_writers = None;
    for start in (0..entries.len()).step_by(FINAL_DICT_WRITE_BATCH_ROWS) {
        let end = start
            .saturating_add(FINAL_DICT_WRITE_BATCH_ROWS)
            .min(entries.len());
        let array = dictionary_array(type_id, column, entries, start..end)?;
        let leaves = compute_leaves(field, &array)?;
        if parquet_writers.is_none() {
            parquet_writers = Some(
                (0..leaves.len())
                    .map(|_index| column_writers.next().ok_or(CodecError::SchemaMismatch))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let writers = parquet_writers.as_mut().ok_or(CodecError::SchemaMismatch)?;
        if writers.len() != leaves.len() {
            return Err(CodecError::SchemaMismatch);
        }
        for (writer, leaf) in writers.iter_mut().zip(leaves) {
            writer.write(&leaf)?;
        }
    }
    let parquet_writers = parquet_writers.ok_or(CodecError::SchemaMismatch)?;
    for writer in parquet_writers {
        writer.close()?.append_to_row_group(row_group)?;
    }
    Ok(())
}

fn dictionary_array(
    type_id: u32,
    column: usize,
    entries: &[EntrySnapshot<'_>],
    rows: Range<usize>,
) -> Result<ArrayRef, CodecError> {
    let entries = entries.get(rows).ok_or(CodecError::SchemaMismatch)?;
    let array: ArrayRef = match (type_id, column) {
        (DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID, 0) => Arc::new(UInt64Array::from_iter_values(
            entries.iter().map(|entry| entry.str_id.get()),
        )),
        (DICT_STRINGS_TYPE_ID, 1) => Arc::new(BinaryArray::from_iter_values(
            entries.iter().map(|entry| entry.stored_bytes),
        )),
        (DICT_BLOBS_TYPE_ID, 1) => Arc::new(BinaryArray::from_iter_values(
            entries.iter().map(|entry| entry.stored_bytes),
        )),
        (DICT_BLOBS_TYPE_ID, 2) => Arc::new(UInt64Array::from_iter_values(
            entries.iter().map(|entry| entry.full_len),
        )),
        (DICT_BLOBS_TYPE_ID, 3) => Arc::new(
            entries
                .iter()
                .map(|entry| Some(entry.truncated))
                .collect::<BooleanArray>(),
        ),
        (DICT_BLOBS_TYPE_ID, 4) => Arc::new(FixedSizeBinaryArray::try_from_sparse_iter_with_size(
            entries.iter().map(|entry| entry.full_sha256),
            32,
        )?),
        _ => return Err(CodecError::SchemaMismatch),
    };
    Ok(array)
}

struct DictSink<W> {
    out: W,
    len: u64,
    checksum: Crc32c,
}

impl<W> DictSink<W> {
    const fn new(out: W) -> Self {
        Self {
            out,
            len: 0,
            checksum: Crc32c::new(),
        }
    }

    fn finish(self) -> (u64, u32) {
        (self.len, self.checksum.finalize())
    }
}

impl<W: Write> Write for DictSink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.out.write(buf)?;
        self.checksum.update(&buf[..written]);
        let written_u64 = u64::try_from(written)
            .map_err(|_overflow| io::Error::other("dictionary section length overflow"))?;
        self.len = self
            .len
            .checked_add(written_u64)
            .ok_or_else(|| io::Error::other("dictionary section length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Write `batch` to a capped zstd Parquet body.
fn section(
    type_id: u32,
    batch: &RecordBatch,
    properties: &WriterProperties,
) -> Result<DictSection, CodecError> {
    if batch.num_rows() > MAX_SECTION_ROWS {
        return Err(CodecError::TooManyRows {
            rows: batch.num_rows(),
            max: MAX_SECTION_ROWS,
        });
    }

    let options = ArrowWriterOptions::new()
        .with_properties(properties.clone())
        .with_skip_arrow_metadata(true);
    let mut body = Vec::new();
    let mut writer = ArrowWriter::try_new_with_options(&mut body, batch.schema(), options)?;
    writer.write(batch)?;
    writer.close()?;

    if body.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: body.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    Ok(DictSection {
        type_id,
        rows: u32::try_from(batch.num_rows()).unwrap_or(u32::MAX),
        body,
    })
}

#[cfg(test)]
mod tests;
