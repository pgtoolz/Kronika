//! Reading a verified section body back into Arrow batches.

use super::{
    Array, ArrowReaderOptions, BooleanArray, Bytes, CodecError, DECODE_BATCH_SIZE,
    MAX_DECODED_SECTION_BYTES, MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder, RecordBatch, row_count_fits,
};

/// Build a Parquet reader after byte, row-group, and claimed-row caps pass.
///
/// Returns row-group and claimed-row counts for stats and preallocation.
pub(super) fn capped_reader(
    bytes: Bytes,
) -> Result<(ParquetRecordBatchReader, usize, usize), CodecError> {
    if bytes.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: bytes.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    crate::validate_parquet_decode_work(bytes.as_ref(), MAX_DECODED_SECTION_BYTES)?;
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(bytes, options)?;

    let groups = builder.metadata().num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        });
    }

    let claimed = builder.metadata().file_metadata().num_rows();
    let row_count = match usize::try_from(claimed) {
        Ok(rows) if row_count_fits(rows) => rows,
        Ok(rows) => {
            return Err(CodecError::TooManyRows {
                rows,
                max: MAX_SECTION_ROWS,
            });
        }
        Err(_) => return Err(CodecError::InvalidRowCount { raw: claimed }),
    };

    Ok((
        builder.with_batch_size(DECODE_BATCH_SIZE).build()?,
        groups,
        row_count,
    ))
}

pub(super) fn boolean_column<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a BooleanArray, CodecError> {
    let column = batch
        .column_by_name(name)
        .ok_or(CodecError::MissingColumn { name })?;
    column
        .as_any()
        .downcast_ref::<BooleanArray>()
        .ok_or(CodecError::ColumnType { name })
}
