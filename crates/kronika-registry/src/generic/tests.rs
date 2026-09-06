use super::{Cell, Row, decode_rows, visit_batches, visit_rows};
use crate::contract::TypeContract;
use crate::os_process::{OsProcess, test_row as process_row};
use crate::{Section, VerifiedSection, arrow_schema, registry};

/// The process contract straight from the registry.
fn process_contract() -> &'static TypeContract {
    registry()
        .iter()
        .find(|contract| contract.type_id.get() == 1_100_001)
        .expect("process contract registered")
}

/// A full cell vector for the process contract: `Null` everywhere except
/// the first column (`ts`).
fn process_cells(ts: i64) -> Vec<Cell> {
    let contract = process_contract();
    let mut cells = vec![Cell::Null; contract.columns.len()];
    cells[0] = Cell::Ts(ts);
    cells
}

// ---- positional Row API ----

#[test]
fn row_get_resolves_each_contract_column_positionally() {
    let contract = process_contract();
    let cells: Vec<Cell> = (0..contract.columns.len())
        .map(|i| Cell::I64(i64::try_from(i).expect("small index")))
        .collect();
    let row = Row::new(contract, cells);
    for (i, column) in contract.columns.iter().enumerate() {
        assert_eq!(
            row.get(column.name),
            Some(&Cell::I64(i64::try_from(i).expect("small index"))),
            "column {} resolves to its position",
            column.name
        );
    }
}

#[test]
fn row_get_is_none_for_a_column_outside_the_contract() {
    let row = Row::new(process_contract(), process_cells(1));
    assert_eq!(row.get("no_such_column"), None);
}

#[test]
fn row_shorter_cells_leave_tail_columns_absent() {
    let contract = process_contract();
    let row = Row::new(contract, vec![Cell::Ts(5)]);
    assert_eq!(row.get(contract.columns[0].name), Some(&Cell::Ts(5)));
    let last = contract
        .columns
        .last()
        .expect("process contract has columns");
    assert_eq!(row.get(last.name), None, "missing tail cell reads absent");
}

#[test]
fn row_iter_follows_contract_column_order() {
    let contract = process_contract();
    let row = Row::new(contract, process_cells(1));
    let names: Vec<&str> = row.iter().map(|(name, _)| name).collect();
    let want: Vec<&str> = contract.columns.iter().map(|column| column.name).collect();
    assert_eq!(names, want);
}

#[test]
fn row_iter_stops_at_the_shorter_of_columns_and_cells() {
    let contract = process_contract();
    let row = Row::new(contract, vec![Cell::Ts(5), Cell::I64(2)]);
    assert_eq!(row.iter().count(), 2, "iter pairs only the present cells");
}

#[test]
fn rows_compare_equal_only_on_same_contract_and_cells() {
    let process = process_contract();
    let row = Row::new(process, process_cells(1));
    assert_eq!(row, Row::new(process, process_cells(1)));
    assert_ne!(row, Row::new(process, process_cells(2)), "cells differ");

    let other = registry()
        .iter()
        .find(|contract| contract.type_id.get() != 1_100_001)
        .expect("registry has more than one contract");
    let foreign = Row::new(other, process_cells(1));
    assert_ne!(row, foreign, "same cells under another contract differ");
}

#[test]
fn decoded_row_cells_align_with_contract_columns() {
    let want = vec![process_row(77, 10, false)];
    let bytes = OsProcess::encode(&want).expect("encode");
    let rows =
        decode_rows(1_100_001, VerifiedSection::for_test(bytes.into())).expect("decode_rows");
    let row = &rows[0];
    let contract = row.contract();
    assert_eq!(contract.type_id.get(), 1_100_001, "contract travels along");
    assert_eq!(
        row.cells().len(),
        contract.columns.len(),
        "decode fills every contract column"
    );
    // The positional view and the by-name view agree cell for cell.
    for (at, column) in contract.columns.iter().enumerate() {
        assert_eq!(
            row.cells().get(at),
            row.get(column.name),
            "cell {} equal by index and by name",
            column.name
        );
    }
}

#[test]
fn projected_visit_applies_offset_limit_and_physical_ordinal() {
    let bytes = OsProcess::encode(&[
        process_row(10, 10, false),
        process_row(20, 20, false),
        process_row(30, 30, false),
    ])
    .expect("encode");
    let mut visited = Vec::new();
    let count = visit_rows(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        &["pid"],
        Some(3),
        1,
        1,
        |ordinal, row| {
            visited.push((ordinal, row));
            true
        },
    )
    .expect("projected visit");
    assert_eq!(count, 1);
    assert_eq!(visited[0].0, 1);
    assert_eq!(visited[0].1.get("pid"), Some(&Cell::I32(20)));
    assert_eq!(visited[0].1.get("ts"), Some(&Cell::Null));
}

#[test]
fn batch_visit_keeps_the_full_contract_schema_and_physical_ordinal() {
    let bytes = OsProcess::encode(&[
        process_row(10, 10, false),
        process_row(20, 20, false),
        process_row(30, 30, false),
    ])
    .expect("encode");
    let mut visited = Vec::new();
    let count = visit_batches(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        None,
        Some(3),
        1,
        2,
        |ordinal, batch| {
            visited.push((ordinal, batch));
            true
        },
    )
    .expect("batch visit");

    assert_eq!(count, 2);
    assert_eq!(visited.len(), 1);
    assert_eq!(visited[0].0, 1);
    let batch = &visited[0].1;
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.schema(), arrow_schema(process_contract()));
    assert_eq!(batch.num_columns(), process_contract().columns.len());
    let pids = batch
        .column_by_name("pid")
        .expect("pid column")
        .as_any()
        .downcast_ref::<arrow_array::Int32Array>()
        .expect("pid is Int32");
    assert_eq!(pids.values(), &[20, 30]);
}

#[test]
fn batch_visit_projects_only_requested_arrow_columns() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false), process_row(20, 20, false)])
        .expect("encode");
    let mut visited = Vec::new();
    let count = visit_batches(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        Some(&["ts"]),
        Some(2),
        0,
        usize::MAX,
        |ordinal, batch| {
            visited.push((ordinal, batch));
            true
        },
    )
    .expect("projected batch visit");

    assert_eq!(count, 2);
    assert_eq!(visited.len(), 1);
    assert_eq!(visited[0].0, 0);
    let batch = &visited[0].1;
    assert_eq!(batch.num_columns(), 1);
    assert!(batch.column_by_name("ts").is_some());
    assert!(batch.column_by_name("pid").is_none());
}

#[test]
fn batch_visit_checks_the_catalog_row_count() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false)]).expect("encode");
    let error = visit_batches(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        None,
        Some(2),
        0,
        usize::MAX,
        |_ordinal, _batch| true,
    )
    .expect_err("mismatched catalog row count");
    assert!(matches!(
        error,
        crate::CodecError::Section { source, .. }
            if matches!(*source, crate::CodecError::RowCountMismatch { expected: 2, got: 1 })
    ));
}

#[test]
fn empty_projection_counts_rows_without_exposing_a_synthetic_column() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false)]).expect("encode");
    visit_rows(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        &[],
        Some(1),
        0,
        1,
        |_ordinal, row| {
            assert!(row.cells().iter().all(|cell| matches!(cell, Cell::Null)));
            true
        },
    )
    .expect("empty projection");
}

#[test]
fn zero_limit_still_rejects_an_unknown_projection() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false)]).expect("encode");
    assert!(
        visit_rows(
            1_100_001,
            VerifiedSection::for_test(bytes.into()),
            &["not_a_column"],
            Some(1),
            0,
            0,
            |_ordinal, _row| true,
        )
        .is_err()
    );
}

#[test]
fn callback_can_stop_before_the_requested_limit() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false), process_row(20, 20, false)])
        .expect("encode");
    let count = visit_rows(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        &["pid"],
        Some(2),
        0,
        2,
        |_ordinal, _row| false,
    )
    .expect("early stop");
    assert_eq!(count, 1);
}

#[test]
fn catalog_and_parquet_row_counts_must_agree() {
    let bytes = OsProcess::encode(&[process_row(10, 10, false)]).expect("encode");
    let error = visit_rows(
        1_100_001,
        VerifiedSection::for_test(bytes.into()),
        &["pid"],
        Some(2),
        0,
        1,
        |_ordinal, _row| true,
    )
    .expect_err("mismatched catalog row count");
    assert!(matches!(
        error,
        crate::CodecError::Section { source, .. }
            if matches!(*source, crate::CodecError::RowCountMismatch { expected: 2, got: 1 })
    ));
}

#[test]
fn roundtrips_every_cell_kind_through_the_process_section() {
    // os_process covers i64 counters, u32/i32/i8/i16/u8 labels, a required
    // StrId, a nullable StrId, and nullable i64 counters in one contract.
    let want = vec![process_row(1_700_000_000_000_000, 10, true), {
        let mut row = process_row(1_700_000_001_000_000, 11, false);
        row.cmdline = None;
        row
    }];
    let bytes = OsProcess::encode(&want).expect("encode");
    let rows =
        decode_rows(1_100_001, VerifiedSection::for_test(bytes.into())).expect("decode_rows");
    assert_eq!(rows.len(), 2, "two rows decode back");

    // Rows are sorted by the `pid` sort key, so pid 10 is first.
    let first = &rows[0];
    assert_eq!(
        first.get("ts"),
        Some(&Cell::Ts(1_700_000_000_000_000)),
        "Ts cell"
    );
    assert_eq!(
        first.get("utime"),
        Some(&Cell::I64(100)),
        "I64 counter cell"
    );
    assert_eq!(
        first.get("cmdline"),
        Some(&Cell::StrId(11)),
        "StrId keeps the raw id, unresolved"
    );
    assert_eq!(
        first.get("syscr"),
        Some(&Cell::I64(1)),
        "present nullable i64"
    );
    assert_eq!(first.get("uid"), Some(&Cell::U32(1000)), "u32 cell");
    assert_eq!(first.get("nice"), Some(&Cell::I16(0)), "i8 widens to I16");

    let second = &rows[1];
    assert_eq!(
        second.get("cmdline"),
        Some(&Cell::Null),
        "absent nullable StrId decodes to Null, distinct from a zero id"
    );
    assert_eq!(
        second.get("read_bytes"),
        Some(&Cell::Null),
        "absent nullable counter"
    );
}

#[test]
fn rejects_an_unregistered_type() {
    assert!(
        decode_rows(2_999_999, VerifiedSection::for_test(bytes::Bytes::new())).is_err(),
        "an unregistered type_id has no contract to decode against"
    );
}

// The process contract has no small-integer, float, or bool column.
// These tests drive
// `cell_at` directly over every other column type, since no registered
// contract carries a small integer, unsigned, float, or bool column.
#[test]
fn cell_at_maps_each_column_type_to_its_widened_cell() {
    use super::cell_at;
    use crate::ColumnType;
    use arrow_array::{
        BooleanArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array, Int64Array,
        UInt8Array, UInt16Array, UInt32Array, UInt64Array,
    };

    assert_eq!(
        cell_at(&Int8Array::from(vec![-5_i8]), ColumnType::I8, "c", 0).unwrap(),
        Cell::I16(-5),
        "i8 widens to I16"
    );
    assert_eq!(
        cell_at(&Int16Array::from(vec![-300_i16]), ColumnType::I16, "c", 0).unwrap(),
        Cell::I16(-300)
    );
    assert_eq!(
        cell_at(&Int32Array::from(vec![70_000_i32]), ColumnType::I32, "c", 0).unwrap(),
        Cell::I32(70_000)
    );
    assert_eq!(
        cell_at(&UInt8Array::from(vec![200_u8]), ColumnType::U8, "c", 0).unwrap(),
        Cell::U32(200),
        "u8 widens to U32"
    );
    assert_eq!(
        cell_at(
            &UInt16Array::from(vec![60_000_u16]),
            ColumnType::U16,
            "c",
            0
        )
        .unwrap(),
        Cell::U32(60_000)
    );
    assert_eq!(
        cell_at(
            &UInt32Array::from(vec![4_000_000_000_u32]),
            ColumnType::U32,
            "c",
            0
        )
        .unwrap(),
        Cell::U32(4_000_000_000)
    );
    assert_eq!(
        cell_at(
            &UInt64Array::from(vec![18_000_000_000_000_000_000_u64]),
            ColumnType::U64,
            "c",
            0
        )
        .unwrap(),
        Cell::U64(18_000_000_000_000_000_000)
    );
    assert_eq!(
        cell_at(&Float32Array::from(vec![1.5_f32]), ColumnType::F32, "c", 0).unwrap(),
        Cell::F64(1.5),
        "f32 widens to F64"
    );
    assert_eq!(
        cell_at(&Float64Array::from(vec![2.25_f64]), ColumnType::F64, "c", 0).unwrap(),
        Cell::F64(2.25)
    );
    assert_eq!(
        cell_at(&BooleanArray::from(vec![true]), ColumnType::Bool, "c", 0).unwrap(),
        Cell::Bool(true)
    );
    assert_eq!(
        cell_at(
            &Int64Array::from(vec![Some(9_i64)]),
            ColumnType::I64,
            "c",
            0
        )
        .unwrap(),
        Cell::I64(9)
    );
}

#[test]
fn cell_at_maps_list_i32_to_a_list_cell() {
    use super::cell_at;
    use crate::ColumnType;
    use arrow_array::ListArray;
    use arrow_array::types::Int32Type;

    let array = ListArray::from_iter_primitive::<Int32Type, _, _>([
        Some(vec![Some(1), Some(2)]),
        Some(vec![]),
    ]);
    assert_eq!(
        cell_at(&array, ColumnType::ListI32, "c", 0).unwrap(),
        Cell::ListI32(vec![1, 2])
    );
    assert_eq!(
        cell_at(&array, ColumnType::ListI32, "c", 1).unwrap(),
        Cell::ListI32(Vec::new())
    );
}

#[test]
fn cell_at_errors_when_the_arrow_type_is_wrong_for_the_column() {
    use super::cell_at;
    use crate::ColumnType;
    use arrow_array::Int32Array;

    // A column declared U64 but backed by an Int32 array cannot downcast.
    let err = cell_at(&Int32Array::from(vec![1_i32]), ColumnType::U64, "c", 0)
        .expect_err("a type mismatch is an error, not a panic");
    assert!(
        matches!(err, crate::CodecError::ColumnType { name: "c" }),
        "the error names the offending column"
    );
}
