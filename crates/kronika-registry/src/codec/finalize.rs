//! Bounded coalescing of journal sections into one finished Parquet body.

use std::cmp::Ordering;
use std::io::Write;
use std::ops::Range;
use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatchReader, UInt32Array};
use arrow_ord::sort::{LexicographicalComparator, SortColumn, lexsort_to_indices};
use arrow_select::concat::concat;
use arrow_select::interleave::interleave;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder,
};
use parquet::arrow::arrow_writer::{
    ArrowColumnWriter, ArrowWriterOptions, compute_leaves, get_column_writers,
};
use parquet::arrow::{ArrowSchemaConverter, ArrowWriter, ProjectionMask};
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use parquet::file::reader::ChunkReader;
use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};

use super::{
    CodecError, ColumnType, DECODE_BATCH_SIZE, FINAL_WRITER_PROPS, MAX_DECODED_SECTION_BYTES,
    MAX_LIST_I32_VALUES_PER_SECTION, MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    ParquetRecordBatchReader, TypeContract, VerifiedSection, arrow_schema, check_row_cap,
    final_data_body_bound, schema_matches, validate_list_i32_batch,
};

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
    let contract = contract(type_id)?;
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
/// body. The finalizer retains the sort-key columns while it establishes row
/// order, then at most one decoded output column, compact row locations, and
/// one output chunk.
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
    let contract = contract(type_id)?;
    let rows = aggregate_rows(expected_rows)?;
    if rows == 0 {
        return Err(CodecError::SchemaMismatch.into());
    }

    let mut metadata = SectionMetadataCache::new(expected_rows.len());
    let mut plan = canonical_order(expected_rows, contract, &mut metadata, &mut open)?;
    if plan.order.is_none() {
        drop(plan);
        return encode_ordered_sections_to(
            type_id,
            expected_rows,
            rows,
            contract,
            out,
            &mut metadata,
            &mut open,
        );
    }
    let locations = canonical_locations(
        expected_rows,
        plan.order.as_ref().ok_or(CodecError::SchemaMismatch)?,
    )?;
    plan.order = None;
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
        let projected = match plan.projected[column_index].take() {
            Some(arrays) if column.ty != ColumnType::ListI32 => ProjectedColumn {
                arrays,
                list_values: 0,
            },
            _ => project_column(
                expected_rows,
                contract,
                column_index,
                (column.ty == ColumnType::ListI32).then_some(column.name),
                column_index + 1 == contract.columns.len(),
                &mut metadata,
                &mut open,
            )?,
        };

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

fn full_section_reader<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    expected_rows: usize,
    metadata: &mut SectionMetadataCache,
    section_index: usize,
) -> Result<ParquetRecordBatchReader, CodecError> {
    let builder = section_reader_builder(
        source,
        contract,
        expected_rows,
        metadata,
        section_index,
        true,
    )?;
    Ok(builder.with_batch_size(DECODE_BATCH_SIZE).build()?)
}

struct SectionMetadataCache {
    sections: Vec<Option<Arc<ParquetMetaData>>>,
    validated: Vec<bool>,
    bytes: usize,
    enabled: bool,
}

impl SectionMetadataCache {
    fn new(sections: usize) -> Self {
        Self {
            sections: (0..sections).map(|_index| None).collect(),
            validated: vec![false; sections],
            bytes: 0,
            enabled: true,
        }
    }

    fn validate<R: ChunkReader>(
        &mut self,
        section_index: usize,
        source: &R,
        len: usize,
    ) -> Result<(), CodecError> {
        let validated = self
            .validated
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if !*validated {
            let body = source.get_bytes(0, len)?;
            crate::validate_parquet_decode_work(body.as_ref(), MAX_DECODED_SECTION_BYTES)?;
            *validated = true;
        }
        Ok(())
    }

    fn get(
        &mut self,
        section_index: usize,
        remove: bool,
    ) -> Result<Option<Arc<ParquetMetaData>>, CodecError> {
        let section = self
            .sections
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if remove {
            let Some(metadata) = section.take() else {
                return Ok(None);
            };
            self.bytes = self
                .bytes
                .checked_sub(metadata.memory_size())
                .ok_or(CodecError::SchemaMismatch)?;
            Ok(Some(metadata))
        } else {
            Ok(section.as_ref().map(Arc::clone))
        }
    }

    fn remember(
        &mut self,
        section_index: usize,
        metadata: &Arc<ParquetMetaData>,
    ) -> Result<(), CodecError> {
        if !self.enabled {
            return Ok(());
        }
        let next_bytes = self
            .bytes
            .checked_add(metadata.memory_size())
            .ok_or(CodecError::SchemaMismatch)?;
        if next_bytes > MAX_CACHED_METADATA_BYTES {
            self.enabled = false;
            return Ok(());
        }
        let slot = self
            .sections
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if slot.is_some() {
            return Err(CodecError::SchemaMismatch);
        }
        *slot = Some(Arc::clone(metadata));
        self.bytes = next_bytes;
        Ok(())
    }

    fn clear(&mut self) {
        self.sections.fill(None);
        self.bytes = 0;
        self.enabled = false;
    }
}

fn section_reader_builder<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    expected_rows: usize,
    cache: &mut SectionMetadataCache,
    section_index: usize,
    remove_metadata: bool,
) -> Result<ParquetRecordBatchReaderBuilder<R>, CodecError> {
    let len = usize::try_from(source.len()).map_err(|_overflow| CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    })?;
    if len > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len,
            max: MAX_SECTION_BYTES,
        });
    }
    cache.validate(section_index, &source, len)?;
    let (metadata, cached) = if let Some(metadata) = cache.get(section_index, remove_metadata)? {
        (metadata, true)
    } else {
        let metadata = Arc::new(ParquetMetaDataReader::new().parse_and_finish(&source)?);
        validate_section_metadata(metadata.as_ref(), expected_rows)?;
        if !remove_metadata {
            cache.remember(section_index, &metadata)?;
        }
        (metadata, false)
    };
    let metadata = if cached {
        let options = ArrowReaderOptions::new().with_schema(arrow_schema(contract));
        ArrowReaderMetadata::try_new(metadata, options)?
    } else {
        let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
        let metadata = ArrowReaderMetadata::try_new(metadata, options)?;
        if !schema_matches(metadata.schema().as_ref(), contract) {
            return Err(CodecError::SchemaMismatch);
        }
        metadata
    };
    Ok(ParquetRecordBatchReaderBuilder::new_with_metadata(
        source, metadata,
    ))
}

fn validate_section_metadata(
    metadata: &ParquetMetaData,
    expected_rows: usize,
) -> Result<(), CodecError> {
    let groups = metadata.num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        });
    }
    let claimed = metadata.file_metadata().num_rows();
    let claimed = usize::try_from(claimed)
        .map_err(|_overflow| CodecError::InvalidRowCount { raw: claimed })?;
    check_row_cap(claimed)?;
    if claimed != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows as u64,
            got: claimed as u64,
        });
    }
    Ok(())
}

fn contract(type_id: u32) -> Result<&'static TypeContract, CodecError> {
    crate::registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })
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

struct CanonicalPlan {
    order: Option<UInt32Array>,
    projected: Vec<Option<Vec<ArrayRef>>>,
}

fn finish_canonical_plan(order: Vec<u32>, projected: Vec<Option<Vec<ArrayRef>>>) -> CanonicalPlan {
    let order = if order
        .iter()
        .enumerate()
        .all(|(expected, &actual)| actual as usize == expected)
    {
        None
    } else {
        Some(UInt32Array::from(order))
    };
    CanonicalPlan { order, projected }
}

fn canonical_column_indices(
    contract: &TypeContract,
) -> Result<(Vec<usize>, Vec<usize>), CodecError> {
    let mut sort_key_indices = Vec::with_capacity(contract.sort_key.len());
    for &name in contract.sort_key {
        let index = contract
            .columns
            .iter()
            .position(|column| column.name == name)
            .ok_or(CodecError::MissingColumn { name })?;
        sort_key_indices.push(index);
    }
    let column_indices = (0..contract.columns.len())
        .filter(|index| !sort_key_indices.contains(index))
        .collect();
    Ok((sort_key_indices, column_indices))
}

fn canonical_order<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    metadata: &mut SectionMetadataCache,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<CanonicalPlan, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let rows = aggregate_rows(expected_rows)?;
    if rows <= 1 || contract.columns.is_empty() {
        return Ok(CanonicalPlan {
            order: None,
            projected: vec![None; contract.columns.len()],
        });
    }

    let (sort_key_indices, column_indices) = canonical_column_indices(contract)?;

    let identity = 0..u32::try_from(rows).map_err(|_overflow| CodecError::TooManyRows {
        rows,
        max: MAX_SECTION_ROWS,
    })?;
    let mut order = identity.collect::<Vec<_>>();
    let mut cached = vec![None; contract.columns.len()];
    let mut ties = if sort_key_indices.is_empty() {
        #[allow(
            clippy::single_range_in_vec_init,
            reason = "the first refinement pass starts with every row tied"
        )]
        let all = vec![0..rows];
        all
    } else {
        let projected =
            project_columns(expected_rows, contract, &sort_key_indices, metadata, open)?;
        let sort_columns = projected
            .values
            .iter()
            .map(|values| SortColumn {
                values: Arc::clone(values),
                options: None,
            })
            .collect::<Vec<_>>();
        let indices = lexsort_to_indices(&sort_columns, None).map_err(CodecError::from)?;
        order.copy_from_slice(indices.values());
        let comparator =
            LexicographicalComparator::try_new(&sort_columns).map_err(CodecError::from)?;
        let mut tied = Vec::new();
        collect_ties(&order, 0..rows, &comparator, &mut tied);
        drop(comparator);
        drop(sort_columns);
        for (column_index, arrays) in sort_key_indices.iter().copied().zip(projected.arrays) {
            cached[column_index] = Some(arrays);
        }
        tied
    };

    for column_index in column_indices {
        if ties.is_empty() {
            break;
        }
        let column = &contract.columns[column_index];
        let projected = project_column(
            expected_rows,
            contract,
            column_index,
            (column.ty == ColumnType::ListI32).then_some(column.name),
            false,
            metadata,
            open,
        )?;
        let arrays = projected
            .arrays
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>();
        let values = concat(&arrays).map_err(CodecError::from)?;
        drop(arrays);
        drop(projected);
        let sort_columns = [SortColumn {
            values,
            options: None,
        }];
        let comparator =
            LexicographicalComparator::try_new(&sort_columns).map_err(CodecError::from)?;
        let mut next_ties = Vec::new();
        for range in ties {
            order[range.clone()]
                .sort_by(|left, right| comparator.compare(*left as usize, *right as usize));
            collect_ties(&order, range, &comparator, &mut next_ties);
        }
        ties = next_ties;
    }

    Ok(finish_canonical_plan(order, cached))
}

fn collect_ties(
    order: &[u32],
    range: Range<usize>,
    comparator: &LexicographicalComparator,
    ties: &mut Vec<Range<usize>>,
) {
    let mut start = range.start;
    for position in range.start.saturating_add(1)..range.end {
        if comparator.compare(order[position - 1] as usize, order[position] as usize)
            != Ordering::Equal
        {
            if position - start > 1 {
                ties.push(start..position);
            }
            start = position;
        }
    }
    if range.end - start > 1 {
        ties.push(start..range.end);
    }
}

struct ProjectedColumns {
    arrays: Vec<Vec<ArrayRef>>,
    values: Vec<ArrayRef>,
}

fn project_columns<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    column_indices: &[usize],
    metadata: &mut SectionMetadataCache,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<ProjectedColumns, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let mut projection_indices = column_indices.to_vec();
    projection_indices.sort_unstable();
    projection_indices.dedup();
    if projection_indices.len() != column_indices.len() {
        return Err(CodecError::SchemaMismatch.into());
    }

    let mut columns = (0..column_indices.len())
        .map(|_index| Vec::<ArrayRef>::new())
        .collect::<Vec<_>>();
    let mut rows = 0_usize;
    for (section_index, &expected) in expected_rows.iter().enumerate() {
        let source = open(section_index)?;
        let builder = section_reader_builder(
            source,
            contract,
            expected as usize,
            metadata,
            section_index,
            false,
        )?;
        let mask =
            ProjectionMask::roots(builder.parquet_schema(), projection_indices.iter().copied());
        let reader = builder
            .with_projection(mask)
            .with_batch_size(DECODE_BATCH_SIZE)
            .build()
            .map_err(CodecError::from)?;
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
            if batch.num_columns() != projection_indices.len() {
                return Err(CodecError::SchemaMismatch.into());
            }
            for (arrays, &column_index) in columns.iter_mut().zip(column_indices) {
                let position = projection_indices
                    .binary_search(&column_index)
                    .map_err(|_missing| CodecError::SchemaMismatch)?;
                arrays.push(Arc::clone(batch.column(position)));
            }
        }
        if section_rows != expected as usize {
            return Err(CodecError::RowCountMismatch {
                expected: u64::from(expected),
                got: section_rows as u64,
            }
            .into());
        }
        rows = rows
            .checked_add(section_rows)
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
    }
    let expected = aggregate_rows(expected_rows)?;
    if rows != expected {
        return Err(CodecError::RowCountMismatch {
            expected: expected as u64,
            got: rows as u64,
        }
        .into());
    }

    let values = columns
        .iter()
        .map(|arrays| {
            let arrays = arrays.iter().map(AsRef::as_ref).collect::<Vec<_>>();
            concat(&arrays).map_err(CodecError::from).map_err(E::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectedColumns {
        arrays: columns,
        values,
    })
}

struct ProjectedColumn {
    arrays: Vec<ArrayRef>,
    list_values: usize,
}

fn project_column<R, E>(
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

fn canonical_locations(
    expected_rows: &[u32],
    order: &UInt32Array,
) -> Result<Vec<usize>, CodecError> {
    let rows = aggregate_rows(expected_rows)?;
    if order.len() != rows {
        return Err(CodecError::RowCountMismatch {
            expected: rows as u64,
            got: order.len() as u64,
        });
    }
    order
        .values()
        .iter()
        .map(|&global| {
            let global = global as usize;
            if global >= rows {
                return Err(CodecError::SchemaMismatch);
            }
            Ok(global)
        })
        .collect()
}

fn write_column<W: Write + Send>(
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
