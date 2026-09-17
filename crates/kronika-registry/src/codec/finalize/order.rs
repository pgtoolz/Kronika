//! Refine row order one column at a time: sort keys first, remaining columns
//! as deterministic tie breakers. Retain only the current column and row indices.

use arrow_array::UInt32Array;
use arrow_ord::sort::{LexicographicalComparator, SortColumn};
use arrow_select::concat::concat;
use parquet::file::reader::ChunkReader;
use std::cmp::Ordering;
use std::ops::Range;

use super::{aggregate_rows, columns::project_column, input::SectionMetadataCache};
use crate::codec::{CodecError, ColumnType, MAX_SECTION_ROWS, TypeContract};

fn finish_canonical_order(order: Vec<u32>) -> Option<UInt32Array> {
    if order
        .iter()
        .enumerate()
        .all(|(expected, &actual)| actual as usize == expected)
    {
        None
    } else {
        Some(UInt32Array::from(order))
    }
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

pub(super) fn canonical_order<R, E>(
    expected_rows: &[u32],
    contract: &TypeContract,
    metadata: &mut SectionMetadataCache,
    open: &mut impl FnMut(usize) -> Result<R, E>,
) -> Result<Option<UInt32Array>, E>
where
    R: ChunkReader + 'static,
    E: From<CodecError>,
{
    let rows = aggregate_rows(expected_rows)?;
    if rows <= 1 || contract.columns.is_empty() {
        return Ok(None);
    }

    let (sort_key_indices, column_indices) = canonical_column_indices(contract)?;

    let identity = 0..u32::try_from(rows).map_err(|_overflow| CodecError::TooManyRows {
        rows,
        max: MAX_SECTION_ROWS,
    })?;
    let mut order = identity.collect::<Vec<_>>();
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "the first refinement pass starts with every row tied"
    )]
    let mut ties = vec![0..rows];

    for column_index in sort_key_indices.into_iter().chain(column_indices) {
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

    Ok(finish_canonical_order(order))
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

pub(super) fn canonical_locations(
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
