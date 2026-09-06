//! Size bounds the final Parquet profile must stay inside.

use super::{
    CodecError, ColumnType, FINAL_DATA_PAGE_BYTES, FINAL_FILE_FRAMING_BOUND,
    FINAL_PAGE_FRAMING_BOUND, MAX_LIST_I32_VALUES_PER_SECTION, MAX_SECTION_BYTES,
    SECTION_WRITE_BATCH_ROWS, check_row_cap,
};

/// PLAIN value and level bytes for one physical column before Zstandard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalPlainColumnSize {
    value_bytes: usize,
    level_bytes: usize,
}

impl FinalPlainColumnSize {
    /// Describe one physical PLAIN column for final-body admission.
    #[must_use]
    pub const fn new(value_bytes: usize, level_bytes: usize) -> Self {
        Self {
            value_bytes,
            level_bytes,
        }
    }
}

/// Conservative body bound for PLAIN columns written in one record batch.
///
/// The caller supplies the maximum data-page count per physical column. The
/// bound sums Zstandard's per-page overhead and fixed Parquet framing without
/// assuming how the batch's bytes are distributed between pages.
///
/// # Errors
///
/// Returns [`CodecError::InvalidPageLayout`] for a zero page count, or
/// [`CodecError::SectionTooLarge`] when arithmetic or the conservative final
/// body bound crosses [`MAX_SECTION_BYTES`].
pub fn final_single_batch_plain_body_bound(
    columns: impl IntoIterator<Item = FinalPlainColumnSize>,
    pages_per_column: usize,
) -> Result<usize, CodecError> {
    if pages_per_column == 0 {
        return Err(CodecError::InvalidPageLayout);
    }
    let compression_overhead =
        pages_per_column
            .checked_mul(64)
            .ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?;
    let page_framing = pages_per_column
        .checked_mul(FINAL_PAGE_FRAMING_BOUND)
        .ok_or(CodecError::SectionTooLarge {
            len: usize::MAX,
            max: MAX_SECTION_BYTES,
        })?;
    let mut body = FINAL_FILE_FRAMING_BOUND;
    for column in columns {
        let page_input = column.value_bytes.checked_add(column.level_bytes).ok_or(
            CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            },
        )?;
        let compressed = page_input
            .checked_add(page_input >> 8)
            .and_then(|bytes| bytes.checked_add(compression_overhead))
            .ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?;
        body = body
            .checked_add(compressed)
            .and_then(|bytes| bytes.checked_add(page_framing))
            .ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?;
    }
    if body > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: body,
            max: MAX_SECTION_BYTES,
        });
    }
    Ok(body)
}

/// The `ZSTD_COMPRESSBOUND` formula from the pinned Zstandard 1.5 contract.
#[cfg(test)]
pub(super) fn zstd_compress_bound(src_size: usize) -> Option<usize> {
    let small_input_margin = if src_size < 128 * 1024 {
        ((128 * 1024) - src_size) >> 11
    } else {
        0
    };
    src_size
        .checked_add(src_size >> 8)
        .and_then(|bytes| bytes.checked_add(small_input_margin))
}

/// Prove the page and final-body bounds for one registered final section.
///
/// `list_i32_child_values` is the aggregate child count reported by the
/// generated section codec. Current contracts have at most one list column;
/// assigning the aggregate to each list is conservative if another is added.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown type, row/list overflow, arithmetic
/// overflow, or a version-1 final-body bound breach.
pub fn final_data_body_bound(
    type_id: u32,
    rows: usize,
    list_i32_child_values: usize,
) -> Result<usize, CodecError> {
    check_row_cap(rows)?;
    let contract = crate::registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?;
    let list_name = contract
        .columns
        .iter()
        .find(|column| column.ty == ColumnType::ListI32)
        .map_or("ListI32", |column| column.name);
    if list_i32_child_values > MAX_LIST_I32_VALUES_PER_SECTION {
        return Err(CodecError::TooManyListValues {
            name: list_name,
            values: list_i32_child_values,
            max: MAX_LIST_I32_VALUES_PER_SECTION,
        });
    }
    if list_i32_child_values != 0
        && !contract
            .columns
            .iter()
            .any(|column| column.ty == ColumnType::ListI32)
    {
        return Err(CodecError::SchemaMismatch);
    }

    let mut columns = Vec::with_capacity(contract.columns.len());
    let mut pages_per_column = rows.div_ceil(SECTION_WRITE_BATCH_ROWS).max(1);
    for column in contract.columns {
        let (value_bytes, level_bytes) = if column.ty == ColumnType::ListI32 {
            let values =
                list_i32_child_values
                    .checked_mul(4)
                    .ok_or(CodecError::SectionTooLarge {
                        len: usize::MAX,
                        max: MAX_SECTION_BYTES,
                    })?;
            let levels = rows
                .checked_add(list_i32_child_values)
                .and_then(|count| count.checked_mul(4))
                .and_then(|bytes| bytes.checked_add(16))
                .ok_or(CodecError::SectionTooLarge {
                    len: usize::MAX,
                    max: MAX_SECTION_BYTES,
                })?;
            (values, levels)
        } else {
            let width = match column.ty {
                ColumnType::I8
                | ColumnType::I16
                | ColumnType::I32
                | ColumnType::U8
                | ColumnType::U16
                | ColumnType::U32
                | ColumnType::F32 => 4,
                ColumnType::I64
                | ColumnType::U64
                | ColumnType::F64
                | ColumnType::Ts
                | ColumnType::StrId => 8,
                ColumnType::Bool => 1,
                ColumnType::ListI32 => unreachable!("handled above"),
            };
            let values = rows.checked_mul(width).ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?;
            let levels = if column.nullable {
                rows.checked_mul(2)
                    .and_then(|bytes| bytes.checked_add(8))
                    .ok_or(CodecError::SectionTooLarge {
                        len: usize::MAX,
                        max: MAX_SECTION_BYTES,
                    })?
            } else {
                0
            };
            (values, levels)
        };
        let column_pages = value_bytes
            .checked_add(level_bytes)
            .ok_or(CodecError::SectionTooLarge {
                len: usize::MAX,
                max: MAX_SECTION_BYTES,
            })?
            .div_ceil(FINAL_DATA_PAGE_BYTES)
            .max(1);
        pages_per_column = pages_per_column.max(column_pages);
        columns.push(FinalPlainColumnSize::new(value_bytes, level_bytes));
    }
    final_single_batch_plain_body_bound(columns, pages_per_column)
}
