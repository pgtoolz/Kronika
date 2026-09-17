//! Streaming final dictionary columns while retaining at most one bounded batch.

use std::io::{self, Write};
use std::ops::Range;
use std::sync::Arc;

use arrow_array::{ArrayRef, BinaryArray, BooleanArray, FixedSizeBinaryArray, UInt64Array};
use arrow_schema::Field;
use kronika_format::{Crc32c, EntrySnapshot};
use kronika_registry::{CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_SECTION_BYTES};
use parquet::arrow::ArrowSchemaConverter;
use parquet::arrow::arrow_writer::{ArrowColumnWriter, compute_leaves, get_column_writers};
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};

use super::{
    FINAL_DICT_WRITE_BATCH_ROWS, FINAL_DICT_WRITER_PROPS, WrittenDictSection, check_dict_rows,
    dictionary_schema,
};

pub(super) fn write_final_dictionary(
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
        (DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID, 1) => Arc::new(BinaryArray::from_iter_values(
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
