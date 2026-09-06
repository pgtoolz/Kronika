//! Generic, column-name-addressable section decode.
//!
//! [`visit_rows`] is the production decode path: it projects named Parquet
//! columns, applies a physical row range, and visits one row at a time.
//! [`decode_rows`] is the collecting compatibility wrapper used by existing
//! reader and BDD callers. `StrId` cells stay as raw `u64` ids; the caller
//! resolves them through the segment dictionary.

use arrow_array::{
    Array, BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
    ListArray, RecordBatch, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};

use crate::codec::{
    CodecError, DECODE_BATCH_SIZE, MAX_DECODED_SECTION_BYTES, MAX_LIST_I32_VALUES_PER_ROW,
    MAX_LIST_I32_VALUES_PER_SECTION, MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    row_count_fits, schema_matches, validate_list_i32_batch,
};
use crate::contract::{ColumnType, TypeContract};
use crate::{VerifiedSection, registry, validate_parquet_decode_work};

/// One decoded section value, addressed by column name.
///
/// The variants mirror [`ColumnType`]: every on-disk column type has one cell
/// kind. `Ts` carries unix microseconds; `StrId` carries the raw dictionary id
/// (not the resolved bytes). `ListI32` carries list columns such as
/// `pg_locks.blocked_by`. `Null` is a `NULL` cell in a nullable column.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    /// Signed 16-bit integer (also carries `I8`).
    I16(i16),
    /// Signed 32-bit integer.
    I32(i32),
    /// Signed 64-bit integer.
    I64(i64),
    /// Unsigned 32-bit integer (also carries `U8`/`U16`).
    U32(u32),
    /// Unsigned 64-bit integer.
    U64(u64),
    /// 64-bit float (also carries `F32`, widened).
    F64(f64),
    /// Boolean.
    Bool(bool),
    /// Timestamp, unix microseconds.
    Ts(i64),
    /// A dictionary id; resolve through the segment dictionary for the string.
    StrId(u64),
    /// A list of signed 32-bit integers.
    ListI32(Vec<i32>),
    /// A `NULL` in a nullable column.
    Null,
}

/// A decoded section row: cells in contract column order, addressable by name.
///
/// Cells sit in a vector positionally aligned with the contract's columns, so
/// decode is a straight per-column push with no per-cell map insert. Name
/// lookups walk the contract's column list.
#[derive(Debug, Clone)]
pub struct Row {
    /// The contract the row was decoded against; names cells positionally.
    contract: &'static TypeContract,
    /// One cell per contract column, in contract column order.
    cells: Vec<Cell>,
}

impl PartialEq for Row {
    /// Rows are equal when decoded against the same registry contract (compared
    /// by address — contracts are registry statics) with equal cells.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.contract, other.contract) && self.cells == other.cells
    }
}

impl Row {
    /// Assemble a row of `cells` in `contract` column order.
    ///
    /// The decode path always supplies one cell per column; a shorter vector
    /// leaves the tail columns absent (`get` returns `None`).
    #[must_use]
    pub const fn new(contract: &'static TypeContract, cells: Vec<Cell>) -> Self {
        Self { contract, cells }
    }

    /// The cell under `name`, or `None` when the contract lacks that column.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Cell> {
        let at = self
            .contract
            .columns
            .iter()
            .position(|column| column.name == name)?;
        self.cells.get(at)
    }

    /// The contract this row was decoded against.
    #[must_use]
    pub const fn contract(&self) -> &'static TypeContract {
        self.contract
    }

    /// Cells in contract column order.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// `(column name, cell)` pairs in contract column order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &Cell)> {
        self.contract
            .columns
            .iter()
            .map(|column| column.name)
            .zip(&self.cells)
    }
}

/// Decode a verified section into generic, column-addressable rows.
///
/// The contract is selected by `type_id`. Each row carries one [`Cell`] per
/// contract column; `StrId` columns keep the raw dictionary id.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown `type_id`, a schema mismatch, a cap
/// breach, or a Parquet decode failure — the same failures as [`decode_any`].
///
/// [`decode_any`]: crate::decode_any
pub fn decode_rows(type_id: u32, section: VerifiedSection) -> Result<Vec<Row>, CodecError> {
    let columns: Vec<&str> = registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?
        .columns
        .iter()
        .map(|column| column.name)
        .collect();
    let mut rows = Vec::new();
    visit_rows(
        type_id,
        section,
        &columns,
        None,
        0,
        usize::MAX,
        |_ordinal, row| {
            rows.push(row);
            true
        },
    )?;
    Ok(rows)
}

/// Visit a projected physical row range without collecting a section.
///
/// `columns` are exact contract column names. Cells outside the projection are
/// represented as [`Cell::Null`] and must not be observed by the caller. The
/// callback receives the stable zero-based physical row ordinal and returns
/// `false` to stop decoding early.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown type or column, a schema mismatch,
/// resource caps, or a Parquet decode failure.
pub fn visit_rows(
    type_id: u32,
    section: VerifiedSection,
    columns: &[&str],
    expected_rows: Option<u64>,
    offset: usize,
    limit: usize,
    mut visitor: impl FnMut(u64, Row) -> bool,
) -> Result<usize, CodecError> {
    let contract = registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?;
    let bytes_in = section.len();
    visit_rows_with(
        contract,
        section,
        columns,
        expected_rows,
        offset,
        limit,
        &mut visitor,
    )
    .map_err(|source| CodecError::Section {
        type_id,
        bytes_in,
        source: Box::new(source),
    })
}

/// Visit a physical row range as Arrow record batches.
///
/// `columns` selects exact contract column names. `None` retains the complete
/// contract schema. The callback receives the stable zero-based physical
/// ordinal of the batch's first row and returns `false` to stop after that
/// batch. This is the streaming path for consumers that filter or re-encode
/// rows without materializing [`Cell`]s.
///
/// # Errors
///
/// Returns [`CodecError`] for an unknown type, a schema or row-count mismatch,
/// a resource-cap breach, or a Parquet decode failure.
pub fn visit_batches(
    type_id: u32,
    section: VerifiedSection,
    columns: Option<&[&str]>,
    expected_rows: Option<u64>,
    offset: usize,
    limit: usize,
    mut visitor: impl FnMut(u64, RecordBatch) -> bool,
) -> Result<usize, CodecError> {
    let contract = registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
        .ok_or(CodecError::UnknownType { type_id })?;
    let bytes_in = section.len();
    visit_batches_with(
        contract,
        section,
        columns,
        expected_rows,
        offset,
        limit,
        &mut visitor,
    )
    .map_err(|source| CodecError::Section {
        type_id,
        bytes_in,
        source: Box::new(source),
    })
}

fn visit_batches_with(
    contract: &'static TypeContract,
    section: VerifiedSection,
    columns: Option<&[&str]>,
    expected_rows: Option<u64>,
    offset: usize,
    limit: usize,
    visitor: &mut impl FnMut(u64, RecordBatch) -> bool,
) -> Result<usize, CodecError> {
    if limit == 0 {
        return Ok(0);
    }
    let projection = columns
        .map(|columns| projected_columns(contract, columns))
        .transpose()?;
    let reader = section_reader(
        contract,
        section,
        expected_rows,
        offset,
        limit,
        projection.as_deref(),
    )?;
    let list_columns: Vec<&'static str> = contract
        .columns
        .iter()
        .enumerate()
        .filter(|(index, column)| {
            column.ty == ColumnType::ListI32
                && projection
                    .as_ref()
                    .is_none_or(|projection| projection.contains(index))
        })
        .map(|(_index, column)| column.name)
        .collect();
    let mut list_child_values = vec![0_usize; list_columns.len()];
    let mut visited = 0_usize;
    for batch in reader {
        let batch = batch?;
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
        let ordinal = offset.saturating_add(visited) as u64;
        visited = visited
            .checked_add(batch.num_rows())
            .ok_or(CodecError::TooManyRows {
                rows: usize::MAX,
                max: MAX_SECTION_ROWS,
            })?;
        if !visitor(ordinal, batch) {
            return Ok(visited);
        }
    }
    Ok(visited)
}

fn visit_rows_with(
    contract: &'static TypeContract,
    section: VerifiedSection,
    columns: &[&str],
    expected_rows: Option<u64>,
    offset: usize,
    limit: usize,
    visitor: &mut impl FnMut(u64, Row) -> bool,
) -> Result<usize, CodecError> {
    let requested = projected_columns(contract, columns)?;
    if limit == 0 {
        return Ok(0);
    }
    let physical_projection = if requested.is_empty() {
        vec![0]
    } else {
        requested.clone()
    };

    let reader = section_reader(
        contract,
        section,
        expected_rows,
        offset,
        limit,
        Some(&physical_projection),
    )?;

    let mut visited = 0_usize;
    for batch in reader {
        let batch = batch?;
        let arrays: Vec<(usize, &dyn Array)> = requested
            .iter()
            .map(|&at| {
                let column = &contract.columns[at];
                batch
                    .column_by_name(column.name)
                    .map(|array| (at, array.as_ref()))
                    .ok_or(CodecError::MissingColumn { name: column.name })
            })
            .collect::<Result<_, _>>()?;
        for i in 0..batch.num_rows() {
            let mut cells = vec![Cell::Null; contract.columns.len()];
            for &(at, array) in &arrays {
                let column = &contract.columns[at];
                cells[at] = cell_at(array, column.ty, column.name, i)?;
            }
            let ordinal = offset.saturating_add(visited) as u64;
            visited = visited.saturating_add(1);
            if !visitor(ordinal, Row { contract, cells }) {
                return Ok(visited);
            }
        }
    }
    Ok(visited)
}

fn projected_columns(contract: &TypeContract, columns: &[&str]) -> Result<Vec<usize>, CodecError> {
    let mut requested: Vec<usize> = columns
        .iter()
        .map(|name| {
            contract
                .columns
                .iter()
                .position(|column| column.name == *name)
                .ok_or(CodecError::MissingColumn { name: "projection" })
        })
        .collect::<Result<_, _>>()?;
    requested.sort_unstable();
    requested.dedup();
    Ok(requested)
}

fn section_reader(
    contract: &TypeContract,
    section: VerifiedSection,
    expected_rows: Option<u64>,
    offset: usize,
    limit: usize,
    projection: Option<&[usize]>,
) -> Result<ParquetRecordBatchReader, CodecError> {
    let bytes = section.into_bytes();
    if bytes.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: bytes.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    validate_parquet_decode_work(bytes.as_ref(), MAX_DECODED_SECTION_BYTES)?;
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(bytes, options)?;
    if !schema_matches(builder.schema(), contract) {
        return Err(CodecError::SchemaMismatch);
    }
    let groups = builder.metadata().num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        });
    }
    let claimed = builder.metadata().file_metadata().num_rows();
    let rows = usize::try_from(claimed)
        .map_err(|_overflow| CodecError::InvalidRowCount { raw: claimed })?;
    if !row_count_fits(rows) {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        });
    }
    let rows_u64 =
        u64::try_from(rows).map_err(|_overflow| CodecError::InvalidRowCount { raw: claimed })?;
    if let Some(expected) = expected_rows
        && expected != rows_u64
    {
        return Err(CodecError::RowCountMismatch {
            expected,
            got: rows_u64,
        });
    }
    let builder = if let Some(projection) = projection {
        let mask = ProjectionMask::roots(builder.parquet_schema(), projection.iter().copied());
        builder.with_projection(mask)
    } else {
        builder
    };
    Ok(builder
        .with_offset(offset)
        .with_limit(limit)
        .with_batch_size(DECODE_BATCH_SIZE)
        .build()?)
}

/// Read cell `i` of `array` as a [`Cell`], per the column's [`ColumnType`].
fn cell_at(
    array: &dyn Array,
    ty: ColumnType,
    name: &'static str,
    i: usize,
) -> Result<Cell, CodecError> {
    if array.is_null(i) {
        return Ok(Cell::Null);
    }
    let cell = match ty {
        ColumnType::I8 => Cell::I16(i16::from(typed::<Int8Array>(array, name)?.value(i))),
        ColumnType::I16 => Cell::I16(typed::<Int16Array>(array, name)?.value(i)),
        ColumnType::I32 => Cell::I32(typed::<Int32Array>(array, name)?.value(i)),
        ColumnType::I64 => Cell::I64(typed::<Int64Array>(array, name)?.value(i)),
        ColumnType::U8 => Cell::U32(u32::from(typed::<UInt8Array>(array, name)?.value(i))),
        ColumnType::U16 => Cell::U32(u32::from(typed::<UInt16Array>(array, name)?.value(i))),
        ColumnType::U32 => Cell::U32(typed::<UInt32Array>(array, name)?.value(i)),
        ColumnType::U64 => Cell::U64(typed::<UInt64Array>(array, name)?.value(i)),
        ColumnType::F32 => Cell::F64(f64::from(typed::<Float32Array>(array, name)?.value(i))),
        ColumnType::F64 => Cell::F64(typed::<Float64Array>(array, name)?.value(i)),
        ColumnType::Bool => Cell::Bool(typed::<BooleanArray>(array, name)?.value(i)),
        ColumnType::Ts => Cell::Ts(typed::<Int64Array>(array, name)?.value(i)),
        ColumnType::StrId => Cell::StrId(typed::<UInt64Array>(array, name)?.value(i)),
        ColumnType::ListI32 => Cell::ListI32(list_i32_at(array, name, i)?),
    };
    Ok(cell)
}

/// Read one `List<Int32>` row into an owned vector.
fn list_i32_at(array: &dyn Array, name: &'static str, i: usize) -> Result<Vec<i32>, CodecError> {
    let lists = typed::<ListArray>(array, name)?;
    let values = lists.value(i);
    let ints = values
        .as_any()
        .downcast_ref::<Int32Array>()
        .ok_or(CodecError::ColumnType { name })?;
    if ints.len() > MAX_LIST_I32_VALUES_PER_ROW {
        return Err(CodecError::TooManyListValues {
            name,
            values: ints.len(),
            max: MAX_LIST_I32_VALUES_PER_ROW,
        });
    }
    let mut out = Vec::with_capacity(ints.len());
    for j in 0..ints.len() {
        if ints.is_null(j) {
            return Err(CodecError::NullInRequiredColumn { name });
        }
        out.push(ints.value(j));
    }
    Ok(out)
}

/// Downcast `array` to the concrete Arrow array the column type maps to.
fn typed<'a, A: Array + 'static>(
    array: &'a dyn Array,
    name: &'static str,
) -> Result<&'a A, CodecError> {
    array
        .as_any()
        .downcast_ref::<A>()
        .ok_or(CodecError::ColumnType { name })
}

#[cfg(test)]
mod tests;
