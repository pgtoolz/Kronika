//! Turning contract columns into one Parquet section body.

use super::{
    Arc, ArrowWriter, ArrowWriterOptions, CodecError, ColumnType, ENCODE_BUF_HINT,
    FINAL_WRITER_PROPS, MAX_LIST_I32_VALUES_PER_SECTION, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    RecordBatch, SortColumn, TypeContract, arrow_schema, check_row_cap, concat_batches,
    final_data_body_bound, lexsort_to_indices, schema_matches, take, validate_list_i32_batch,
};

/// Reorder `batch` by the contract's sort-key columns.
pub(super) fn sort_by_sort_key(
    batch: &RecordBatch,
    contract: &TypeContract,
) -> Result<RecordBatch, CodecError> {
    if contract.sort_key.is_empty() || batch.num_rows() <= 1 {
        return Ok(batch.clone());
    }
    let mut sort_columns = Vec::with_capacity(contract.sort_key.len());
    for &name in contract.sort_key {
        let values = batch
            .column_by_name(name)
            .ok_or(CodecError::MissingColumn { name })?;
        sort_columns.push(SortColumn {
            values: Arc::clone(values),
            options: None,
        });
    }
    let indices = lexsort_to_indices(&sort_columns, None)?;
    let columns = batch
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}

/// Coalesce decoded bodies of one registered type into its final ZMS body.
///
/// Input batches may come from any number or order of collection windows. The
/// output is sorted by the registry key and then by every remaining column,
/// encoded as one row group with PLAIN values and Zstandard level 6.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown type, schema mismatch, aggregate row
/// or list bounds, Arrow/Parquet failures, or an encoded body above the
/// section byte cap.
pub fn encode_final_batches(
    type_id: u32,
    mut batches: Vec<RecordBatch>,
) -> Result<Vec<u8>, CodecError> {
    let contract = crate::registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?;
    let schema = arrow_schema(contract);
    let mut rows = 0_usize;
    let list_columns = contract
        .columns
        .iter()
        .filter(|column| column.ty == ColumnType::ListI32)
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let mut list_values = vec![0_usize; list_columns.len()];

    for batch in &batches {
        if !schema_matches(batch.schema().as_ref(), contract) {
            return Err(CodecError::SchemaMismatch);
        }
        rows = rows
            .checked_add(batch.num_rows())
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        check_row_cap(rows)?;
        for (index, &name) in list_columns.iter().enumerate() {
            let values = validate_list_i32_batch(batch, name)?;
            list_values[index] =
                list_values[index]
                    .checked_add(values)
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
                });
            }
        }
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

    let merged = if batches.is_empty() {
        RecordBatch::new_empty(Arc::clone(&schema))
    } else if batches.len() == 1 {
        batches.pop().ok_or(CodecError::SchemaMismatch)?
    } else {
        let merged = concat_batches(&schema, &batches)?;
        drop(batches);
        merged
    };
    let canonical = sort_canonical(merged, contract)?;
    let options = ArrowWriterOptions::new()
        .with_properties(FINAL_WRITER_PROPS.clone())
        .with_skip_arrow_metadata(true);
    let mut body = Vec::with_capacity(ENCODE_BUF_HINT);
    let mut writer = ArrowWriter::try_new_with_options(&mut body, schema, options)?;
    writer.write(&canonical)?;
    writer.close()?;
    if body.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: body.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    Ok(body)
}

/// Apply a deterministic total column order after the registry sort key.
pub(super) fn sort_canonical(
    batch: RecordBatch,
    contract: &TypeContract,
) -> Result<RecordBatch, CodecError> {
    if contract.columns.is_empty() || batch.num_rows() <= 1 {
        return Ok(batch);
    }
    let mut names = contract.sort_key.to_vec();
    names.extend(
        contract
            .columns
            .iter()
            .map(|column| column.name)
            .filter(|name| !contract.sort_key.contains(name)),
    );
    let mut sort_columns = Vec::with_capacity(names.len());
    for name in names {
        let values = batch
            .column_by_name(name)
            .ok_or(CodecError::MissingColumn { name })?;
        sort_columns.push(SortColumn {
            values: Arc::clone(values),
            options: None,
        });
    }
    let indices = lexsort_to_indices(&sort_columns, None)?;
    if indices
        .values()
        .iter()
        .enumerate()
        .all(|(expected, &actual)| actual as usize == expected)
    {
        return Ok(batch);
    }
    let columns = batch
        .columns()
        .iter()
        .map(|column| take(column.as_ref(), &indices, None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RecordBatch::try_new(batch.schema(), columns)?)
}
