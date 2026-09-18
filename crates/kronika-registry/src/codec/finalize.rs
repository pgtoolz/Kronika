//! Coalesce journal sections into one bounded Parquet body in canonical row order.

use std::io::Write;
use std::sync::Arc;

use arrow_array::RecordBatchReader;
use parquet::arrow::arrow_writer::{ArrowWriterOptions, get_column_writers};
use parquet::arrow::{ArrowSchemaConverter, ArrowWriter};
use parquet::file::reader::ChunkReader;
use parquet::file::writer::SerializedFileWriter;

use super::{
    CodecError, ColumnType, FINAL_WRITER_PROPS, MAX_LIST_I32_VALUES_PER_SECTION, MAX_SECTION_ROWS,
    TypeContract, VerifiedSection, arrow_schema, check_row_cap, final_data_body_bound,
    schema_matches, validate_list_i32_batch,
};

mod columns;
mod input;
mod order;

use columns::{project_column, write_column};
use input::{SectionMetadataCache, full_section_reader};
use order::{canonical_locations, canonical_order};

/// Retained Parquet metadata per finalization pass; extra footers are read again
/// for each column instead of growing the cache beyond 2 MiB.
pub(super) const MAX_CACHED_METADATA_BYTES: usize = 2 * 1024 * 1024;

/// Validate one input body before the bounded finalizer reopens it by range.
///
/// This consumes and releases the compressed body after checking its CRC was
/// already verified, its bounded Parquet profile, schema, and declared rows.
///
/// # Errors
///
/// Returns [`CodecError`] when the type is unknown or the section violates
/// its bounded Parquet, schema, or row-count contract.
pub fn validate_final_section(
    type_id: u32,
    section: VerifiedSection,
    expected_rows: u32,
) -> Result<(), CodecError> {
    let contract = crate::contract(type_id).ok_or(CodecError::UnknownType { type_id })?;
    let (reader, _row_groups, rows) = super::decode::capped_reader(section.into_bytes())?;
    if !schema_matches(&reader.schema(), contract) {
        return Err(CodecError::SchemaMismatch);
    }
    if rows != expected_rows as usize {
        return Err(CodecError::RowCountMismatch {
            expected: u64::from(expected_rows),
            got: rows as u64,
        });
    }
    Ok(())
}

/// Encode validated input sections into one final Parquet body.
///
/// `open` returns a fresh bounded random-access view of one compressed input
/// body. The finalizer establishes row order one column at a time, then retains
/// one decoded output column, compact row locations, and one output chunk.
///
/// # Errors
///
/// Returns `E` when opening an input fails, the input violates its registered
/// contract, canonical ordering fails, or the final Parquet body cannot be
/// written.
pub fn encode_final_sections_to<W, R, E>(
    type_id: u32,
    expected_rows: &[u32],
    out: &mut W,
    mut open: impl FnMut(usize) -> Result<R, E>,
) -> Result<(), E>
where
    W: Write + Send,
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let contract = crate::contract(type_id).ok_or(CodecError::UnknownType { type_id })?;
    let rows = aggregate_rows(expected_rows)?;
    if rows == 0 {
        return Err(CodecError::SchemaMismatch.into());
    }

    let mut metadata = SectionMetadataCache::new(expected_rows.len());
    let order = canonical_order(expected_rows, contract, &mut metadata, &mut open)?;
    let Some(order) = order else {
        return encode_ordered_sections_to(
            type_id,
            expected_rows,
            rows,
            contract,
            out,
            &mut metadata,
            &mut open,
        );
    };
    let locations = canonical_locations(expected_rows, &order)?;
    drop(order);
    let schema = arrow_schema(contract);
    let properties = Arc::new(FINAL_WRITER_PROPS.clone());
    let parquet_schema = ArrowSchemaConverter::new()
        .with_coerce_types(properties.coerce_types())
        .convert(&schema)
        .map_err(CodecError::from)?;
    let column_writers =
        get_column_writers(&parquet_schema, &properties, &schema).map_err(CodecError::from)?;
    let mut file = SerializedFileWriter::new(
        out,
        parquet_schema.root_schema_ptr(),
        Arc::clone(&properties),
    )
    .map_err(CodecError::from)?;
    let mut row_group = file.next_row_group().map_err(CodecError::from)?;
    let mut column_writers = column_writers.into_iter();
    let mut total_list_values = 0_usize;

    let mut column_index = 0_usize;
    while column_index < contract.columns.len() {
        let column = &contract.columns[column_index];
        let projected = project_column(
            expected_rows,
            contract,
            column_index,
            (column.ty == ColumnType::ListI32).then_some(column.name),
            column_index + 1 == contract.columns.len(),
            &mut metadata,
            &mut open,
        )?;

        let field = &schema.fields()[column_index];
        if column.ty == ColumnType::ListI32 {
            if projected.list_values > MAX_LIST_I32_VALUES_PER_SECTION {
                return Err(CodecError::TooManyListValues {
                    name: column.name,
                    values: projected.list_values,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                }
                .into());
            }
            total_list_values = total_list_values.checked_add(projected.list_values).ok_or(
                CodecError::TooManyListValues {
                    name: column.name,
                    values: usize::MAX,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                },
            )?;
        }
        write_column(
            field,
            projected.arrays,
            Some(&locations),
            &mut column_writers,
            &mut row_group,
        )?;
        column_index += 1;
    }

    metadata.clear();
    final_data_body_bound(type_id, rows, total_list_values)?;
    if column_writers.next().is_some() {
        return Err(CodecError::SchemaMismatch.into());
    }
    row_group.close().map_err(CodecError::from)?;
    file.close().map_err(CodecError::from)?;
    Ok(())
}

/// Stream sections that are already in canonical order into the final writer.
///
/// The writer may retain its bounded row-group state, but each decoded input
/// batch is released before the next one is opened. Sections that need any
/// reordering continue through the column-at-a-time path above.
fn encode_ordered_sections_to<W, R, E>(
    type_id: u32,
    expected_rows: &[u32],
    rows: usize,
    contract: &TypeContract,
    out: &mut W,
    metadata: &mut SectionMetadataCache,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<(), E>
where
    W: Write + Send,
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let schema = arrow_schema(contract);
    let options = ArrowWriterOptions::new()
        .with_properties(FINAL_WRITER_PROPS.clone())
        .with_skip_arrow_metadata(true);
    let mut writer = ArrowWriter::try_new_with_options(out, Arc::clone(&schema), options)
        .map_err(CodecError::from)?;
    let list_columns = contract
        .columns
        .iter()
        .filter(|column| column.ty == ColumnType::ListI32)
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let mut list_values = vec![0_usize; list_columns.len()];
    let mut observed_rows = 0_usize;

    for (section_index, &expected) in expected_rows.iter().enumerate() {
        let source = open(section_index)?;
        let reader =
            full_section_reader(source, contract, expected as usize, metadata, section_index)?;
        let mut section_rows = 0_usize;
        for batch in reader {
            let batch = batch.map_err(CodecError::from)?;
            section_rows =
                section_rows
                    .checked_add(batch.num_rows())
                    .ok_or(CodecError::TooManyRows {
                        rows: usize::MAX,
                        max: MAX_SECTION_ROWS,
                    })?;
            observed_rows =
                observed_rows
                    .checked_add(batch.num_rows())
                    .ok_or(CodecError::TooManyRows {
                        rows: usize::MAX,
                        max: MAX_SECTION_ROWS,
                    })?;
            for (index, &name) in list_columns.iter().enumerate() {
                list_values[index] = list_values[index]
                    .checked_add(validate_list_i32_batch(&batch, name)?)
                    .ok_or(CodecError::TooManyListValues {
                        name,
                        values: usize::MAX,
                        max: MAX_LIST_I32_VALUES_PER_SECTION,
                    })?;
                if list_values[index] > MAX_LIST_I32_VALUES_PER_SECTION {
                    return Err(CodecError::TooManyListValues {
                        name,
                        values: list_values[index],
                        max: MAX_LIST_I32_VALUES_PER_SECTION,
                    }
                    .into());
                }
            }
            writer.write(&batch).map_err(CodecError::from)?;
        }
        if section_rows != expected as usize {
            return Err(CodecError::RowCountMismatch {
                expected: u64::from(expected),
                got: section_rows as u64,
            }
            .into());
        }
    }
    if observed_rows != rows {
        return Err(CodecError::RowCountMismatch {
            expected: rows as u64,
            got: observed_rows as u64,
        }
        .into());
    }
    let total_list_values = list_values.iter().try_fold(0_usize, |total, &values| {
        total
            .checked_add(values)
            .ok_or(CodecError::TooManyListValues {
                name: list_columns.first().copied().unwrap_or("ListI32"),
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            })
    })?;
    final_data_body_bound(type_id, rows, total_list_values)?;
    writer.close().map_err(CodecError::from)?;
    Ok(())
}

fn aggregate_rows(expected_rows: &[u32]) -> Result<usize, CodecError> {
    let rows = expected_rows
        .iter()
        .try_fold(0_usize, |rows, &additional| {
            rows.checked_add(additional as usize)
                .ok_or(CodecError::TooManyRows {
                    rows: usize::MAX,
                    max: MAX_SECTION_ROWS,
                })
        })?;
    check_row_cap(rows)?;
    Ok(rows)
}
