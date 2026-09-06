//! Building and reading the Arrow arrays behind one contract column.

use super::{
    Arc, Array, ArrayRef, ArrowPrimitiveType, BooleanArray, CodecError, ColumnType, DataType,
    Field, HashMap, Int32Builder, Int32Type, LazyLock, ListArray, ListBuilder,
    MAX_LIST_I32_VALUES_PER_ROW, MAX_LIST_I32_VALUES_PER_SECTION, PrimitiveArray, RecordBatch,
    Schema, SchemaRef, TypeContract, boolean_column,
};

/// Arrow schema of a section, in contract column order.
#[must_use]
pub fn arrow_schema(contract: &TypeContract) -> SchemaRef {
    static CACHE: LazyLock<HashMap<u32, SchemaRef>> = LazyLock::new(|| {
        crate::registry()
            .iter()
            .map(|contract| (contract.type_id.get(), build_arrow_schema(contract)))
            .collect()
    });
    CACHE
        .get(&contract.type_id.get())
        .map_or_else(|| build_arrow_schema(contract), Arc::clone)
}

pub(super) fn build_arrow_schema(contract: &TypeContract) -> SchemaRef {
    let fields: Vec<Field> = contract
        .columns
        .iter()
        .map(|column| {
            let data_type = match column.ty {
                ColumnType::I8 => DataType::Int8,
                ColumnType::I16 => DataType::Int16,
                ColumnType::I32 => DataType::Int32,
                ColumnType::I64 | ColumnType::Ts => DataType::Int64,
                ColumnType::U8 => DataType::UInt8,
                ColumnType::U16 => DataType::UInt16,
                ColumnType::U32 => DataType::UInt32,
                ColumnType::U64 | ColumnType::StrId => DataType::UInt64,
                ColumnType::F32 => DataType::Float32,
                ColumnType::F64 => DataType::Float64,
                ColumnType::Bool => DataType::Boolean,
                ColumnType::ListI32 => {
                    DataType::List(Arc::new(Field::new("item", DataType::Int32, false)))
                }
            };
            Field::new(column.name, data_type, column.nullable)
        })
        .collect();
    Arc::new(Schema::new(fields))
}

/// Whether a decoded file's schema matches the contract.
pub(crate) fn schema_matches(got: &Schema, contract: &TypeContract) -> bool {
    let want = arrow_schema(contract);
    got.fields().len() == want.fields().len()
        && got.fields().iter().zip(want.fields()).all(|(g, w)| {
            g.name() == w.name()
                && g.data_type() == w.data_type()
                && g.is_nullable() == w.is_nullable()
        })
}

/// Build a required primitive column from one value per row.
#[must_use]
pub fn write_required<T: ArrowPrimitiveType>(values: impl Iterator<Item = T::Native>) -> ArrayRef {
    Arc::new(PrimitiveArray::<T>::from_iter_values(values))
}

/// Build an Arrow `List<Int32>` column, one list per row.
///
/// Empty vectors become empty lists; required list columns are never `NULL` and
/// never contain `NULL` child values.
///
/// # Errors
/// Returns [`CodecError`] if the child value count exceeds the row or section
/// cap.
pub fn write_list_i32<T: AsRef<[i32]>>(
    name: &'static str,
    rows: impl Iterator<Item = T>,
) -> Result<ArrayRef, CodecError> {
    let item = Arc::new(Field::new("item", DataType::Int32, false));
    let mut builder = ListBuilder::new(Int32Builder::new()).with_field(item);
    let mut total = 0_usize;
    for row in rows {
        let row = row.as_ref();
        if row.len() > MAX_LIST_I32_VALUES_PER_ROW {
            return Err(CodecError::TooManyListValues {
                name,
                values: row.len(),
                max: MAX_LIST_I32_VALUES_PER_ROW,
            });
        }
        total = total
            .checked_add(row.len())
            .ok_or(CodecError::TooManyListValues {
                name,
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            })?;
        if total > MAX_LIST_I32_VALUES_PER_SECTION {
            return Err(CodecError::TooManyListValues {
                name,
                values: total,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            });
        }
        for &value in row {
            builder.values().append_value(value);
        }
        builder.append(true);
    }
    Ok(Arc::new(builder.finish()))
}

/// A decoded `List<Int32>` column.
#[derive(Debug)]
pub struct ListColumn<'a> {
    array: &'a ListArray,
}

impl ListColumn<'_> {
    /// The list at row `i` as an owned `Vec<i32>`.
    ///
    /// # Panics
    ///
    /// Panics if `i` is out of bounds for the column.
    #[must_use]
    pub fn value(&self, i: usize) -> Vec<i32> {
        let values = self.array.value(i);
        let ints = values
            .as_any()
            .downcast_ref::<PrimitiveArray<Int32Type>>()
            .expect("list child is Int32");
        (0..ints.len()).map(|j| ints.value(j)).collect()
    }
}

/// Borrow a `List<Int32>` column by name.
///
/// # Errors
///
/// Returns [`CodecError`] when the column is missing or is not a `List<Int32>`.
pub fn read_list_i32<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<ListColumn<'a>, CodecError> {
    let column = batch
        .column_by_name(name)
        .ok_or(CodecError::MissingColumn { name })?;
    let array = column
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or(CodecError::ColumnType { name })?;
    validate_list_i32_array(array, name)?;
    Ok(ListColumn { array })
}

pub(crate) fn validate_list_i32_batch(
    batch: &RecordBatch,
    name: &'static str,
) -> Result<usize, CodecError> {
    let column = batch
        .column_by_name(name)
        .ok_or(CodecError::MissingColumn { name })?;
    let array = column
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or(CodecError::ColumnType { name })?;
    validate_list_i32_array(array, name)
}

pub(super) fn validate_list_i32_array(
    array: &ListArray,
    name: &'static str,
) -> Result<usize, CodecError> {
    if array.null_count() != 0 {
        return Err(CodecError::NullInRequiredColumn { name });
    }
    let ints = array
        .values()
        .as_any()
        .downcast_ref::<PrimitiveArray<Int32Type>>()
        .ok_or(CodecError::ColumnType { name })?;

    let mut total = 0_usize;
    for i in 0..array.len() {
        let len = usize::try_from(array.value_length(i)).map_err(|_err| {
            CodecError::TooManyListValues {
                name,
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_ROW,
            }
        })?;
        if len > MAX_LIST_I32_VALUES_PER_ROW {
            return Err(CodecError::TooManyListValues {
                name,
                values: len,
                max: MAX_LIST_I32_VALUES_PER_ROW,
            });
        }
        total = total
            .checked_add(len)
            .ok_or(CodecError::TooManyListValues {
                name,
                values: usize::MAX,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            })?;
        if total > MAX_LIST_I32_VALUES_PER_SECTION {
            return Err(CodecError::TooManyListValues {
                name,
                values: total,
                max: MAX_LIST_I32_VALUES_PER_SECTION,
            });
        }
    }
    let offsets = array.value_offsets();
    let start = usize::try_from(offsets[0]).map_err(|_negative| CodecError::ColumnType { name })?;
    let end = usize::try_from(offsets[array.len()])
        .map_err(|_negative| CodecError::ColumnType { name })?;
    for i in start..end {
        if ints.is_null(i) {
            return Err(CodecError::NullInRequiredColumn { name });
        }
    }
    Ok(total)
}

/// Build a nullable primitive column; `None` becomes a `NULL` cell.
#[must_use]
pub fn write_nullable<T: ArrowPrimitiveType>(
    values: impl Iterator<Item = Option<T::Native>>,
) -> ArrayRef {
    Arc::new(values.collect::<PrimitiveArray<T>>())
}

/// Build a required boolean column.
#[must_use]
pub fn write_bool(values: impl Iterator<Item = bool>) -> ArrayRef {
    Arc::new(values.map(Some).collect::<BooleanArray>())
}

/// Build a nullable boolean column.
#[must_use]
pub fn write_bool_nullable(values: impl Iterator<Item = Option<bool>>) -> ArrayRef {
    Arc::new(values.collect::<BooleanArray>())
}

pub(super) fn primitive_column<'a, T: ArrowPrimitiveType>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a PrimitiveArray<T>, CodecError> {
    let column = batch
        .column_by_name(name)
        .ok_or(CodecError::MissingColumn { name })?;
    column
        .as_any()
        .downcast_ref::<PrimitiveArray<T>>()
        .ok_or(CodecError::ColumnType { name })
}

/// A required primitive column; rejects `NULL`.
///
/// # Errors
///
/// Returns [`CodecError`] when the column is missing, has a different type, or
/// contains `NULL`.
pub fn required_column<'a, T: ArrowPrimitiveType>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a PrimitiveArray<T>, CodecError> {
    let array = primitive_column::<T>(batch, name)?;
    if array.null_count() == 0 {
        Ok(array)
    } else {
        Err(CodecError::NullInRequiredColumn { name })
    }
}

/// A nullable primitive column.
///
/// # Errors
///
/// Returns [`CodecError`] when the column is missing or has a different type.
pub fn nullable_column<'a, T: ArrowPrimitiveType>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a PrimitiveArray<T>, CodecError> {
    primitive_column::<T>(batch, name)
}

/// A required boolean column; rejects `NULL`.
///
/// # Errors
///
/// Returns [`CodecError`] when the column is missing, has a different type, or
/// contains `NULL`.
pub fn required_bool<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a BooleanArray, CodecError> {
    let array = boolean_column(batch, name)?;
    if array.null_count() == 0 {
        Ok(array)
    } else {
        Err(CodecError::NullInRequiredColumn { name })
    }
}

/// A nullable boolean column.
///
/// # Errors
///
/// Returns [`CodecError`] when the column is missing or has a different type.
pub fn nullable_bool<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a BooleanArray, CodecError> {
    boolean_column(batch, name)
}

/// Read primitive cell `i` as `Option`, mapping a null cell to `None`.
#[must_use]
pub fn opt_primitive<T: ArrowPrimitiveType>(
    array: &PrimitiveArray<T>,
    i: usize,
) -> Option<T::Native> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i))
    }
}

/// Read boolean cell `i` as `Option`, mapping a null cell to `None`.
#[must_use]
pub fn opt_bool(array: &BooleanArray, i: usize) -> Option<bool> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i))
    }
}
