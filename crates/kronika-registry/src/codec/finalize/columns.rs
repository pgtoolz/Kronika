//! Decode one projected column, then write it in canonical row order in bounded batches.

use arrow_array::ArrayRef;
use arrow_select::interleave::interleave;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_writer::{ArrowColumnWriter, compute_leaves};
use parquet::file::reader::ChunkReader;
use parquet::file::writer::SerializedRowGroupWriter;
use std::io::Write;

use super::{
    aggregate_rows,
    input::{SectionMetadataCache, section_reader_builder},
};
use crate::codec::{
    CodecError, DECODE_BATCH_SIZE, MAX_LIST_I32_VALUES_PER_SECTION, MAX_SECTION_ROWS, TypeContract,
    validate_list_i32_batch,
};

pub(super) struct ProjectedColumn {
    pub(super) arrays: Vec<ArrayRef>,
    pub(super) list_values: usize,
}

pub(super) fn project_column<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    column_index: usize,
    list_name: Option<&'static str>,
    remove_metadata: bool,
    metadata: &mut SectionMetadataCache,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<ProjectedColumn, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let mut arrays = Vec::new();
    let mut rows = 0_usize;
    let mut list_values = 0_usize;
    for (section_index, &expected) in expected_rows.iter().enumerate() {
        let source = open(section_index)?;
        let projected = decode_projected_column(
            source,
            contract,
            column_index,
            expected as usize,
            list_name,
            metadata,
            SectionRead {
                index: section_index,
                remove_metadata,
            },
        )?;
        rows = rows
            .checked_add(projected.rows)
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        list_values = list_values.checked_add(projected.list_values).ok_or(
            CodecError::TooManyListValues {
                name: list_name.unwrap_or("ListI32"),
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            },
        )?;
        arrays.extend(projected.arrays);
    }
    let expected = aggregate_rows(expected_rows)?;
    if rows != expected {
        return Err(CodecError::RowCountMismatch {
            expected: expected as u64,
            got: rows as u64,
        }
        .into());
    }
    Ok(ProjectedColumn {
        arrays,
        list_values,
    })
}

struct SectionColumn {
    arrays: Vec<ArrayRef>,
    rows: usize,
    list_values: usize,
}

#[derive(Clone, Copy)]
struct SectionRead {
    index: usize,
    remove_metadata: bool,
}

fn decode_projected_column<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    column_index: usize,
    expected_rows: usize,
    list_name: Option<&'static str>,
    metadata: &mut SectionMetadataCache,
    read: SectionRead,
) -> Result<SectionColumn, CodecError> {
    let builder = section_reader_builder(
        source,
        contract,
        expected_rows,
        metadata,
        read.index,
        read.remove_metadata,
    )?;

    let mask = ProjectionMask::roots(builder.parquet_schema(), [column_index]);
    let reader = builder
        .with_projection(mask)
        .with_batch_size(DECODE_BATCH_SIZE)
        .build()?;
    let mut arrays = Vec::with_capacity(expected_rows.div_ceil(DECODE_BATCH_SIZE).max(1));
    let mut rows = 0_usize;
    let mut list_values = 0_usize;
    for batch in reader {
        let batch = batch?;
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        if let Some(name) = list_name {
            list_values = list_values
                .checked_add(validate_list_i32_batch(&batch, name)?)
                .ok_or(CodecError::TooManyListValues {
                    name,
                    values: usize::MAX,
                    max: MAX_LIST_I32_VALUES_PER_SECTION,
                })?;
        }
        let (_schema, mut columns, _rows) = batch.into_parts();
        if columns.len() != 1 {
            return Err(CodecError::SchemaMismatch);
        }
        arrays.push(columns.pop().ok_or(CodecError::SchemaMismatch)?);
    }
    if rows != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows as u64,
            got: rows as u64,
        });
    }
    Ok(SectionColumn {
        arrays,
        rows,
        list_values,
    })
}

pub(super) fn write_column<W: Write + Send>(
    field: &arrow_schema::FieldRef,
    arrays: Vec<ArrayRef>,
    locations: Option<&[usize]>,
    column_writers: &mut impl Iterator<Item = ArrowColumnWriter>,
    row_group: &mut SerializedRowGroupWriter<'_, W>,
) -> Result<(), CodecError> {
    let mut parquet_writers = None;
    if let Some(locations) = locations {
        let values = arrays.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        let mut array_ends = Vec::with_capacity(arrays.len());
        let mut rows = 0_usize;
        for array in &arrays {
            rows = rows
                .checked_add(array.len())
                .ok_or(CodecError::TooManyRows {
                    rows: usize::MAX,
                    max: MAX_SECTION_ROWS,
                })?;
            array_ends.push(rows);
        }
        if rows != locations.len() {
            return Err(CodecError::RowCountMismatch {
                expected: locations.len() as u64,
                got: rows as u64,
            });
        }
        for chunk in locations.chunks(DECODE_BATCH_SIZE) {
            let mut batch_locations = Vec::with_capacity(chunk.len());
            for &global in chunk {
                let source = array_ends.partition_point(|&end| end <= global);
                let start = if source == 0 {
                    0
                } else {
                    array_ends[source - 1]
                };
                if source >= arrays.len() || global < start {
                    return Err(CodecError::SchemaMismatch);
                }
                let offset = global - start;
                if offset >= arrays[source].len() {
                    return Err(CodecError::SchemaMismatch);
                }
                batch_locations.push((source, offset));
            }
            let canonical = interleave(&values, &batch_locations)?;
            write_array(field, &canonical, column_writers, &mut parquet_writers)?;
        }
    } else {
        for array in arrays {
            write_array(field, &array, column_writers, &mut parquet_writers)?;
        }
    }

    let parquet_writers = parquet_writers.ok_or(CodecError::SchemaMismatch)?;
    for writer in parquet_writers {
        writer.close()?.append_to_row_group(row_group)?;
    }
    Ok(())
}

fn write_array(
    field: &arrow_schema::FieldRef,
    array: &ArrayRef,
    column_writers: &mut impl Iterator<Item = ArrowColumnWriter>,
    parquet_writers: &mut Option<Vec<ArrowColumnWriter>>,
) -> Result<(), CodecError> {
    let leaves = compute_leaves(field, array)?;
    if parquet_writers.is_none() {
        let writers = (0..leaves.len())
            .map(|_index| column_writers.next().ok_or(CodecError::SchemaMismatch))
            .collect::<Result<Vec<_>, _>>()?;
        *parquet_writers = Some(writers);
    }
    let writers = parquet_writers.as_mut().ok_or(CodecError::SchemaMismatch)?;
    if writers.len() != leaves.len() {
        return Err(CodecError::SchemaMismatch);
    }
    for (writer, leaf) in writers.iter_mut().zip(leaves) {
        writer.write(&leaf)?;
    }
    Ok(())
}
