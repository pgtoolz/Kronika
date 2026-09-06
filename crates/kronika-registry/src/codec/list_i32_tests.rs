use std::sync::Arc;

use arrow_array::ListArray;
use arrow_array::RecordBatch;
use arrow_array::types::Int32Type;
use arrow_schema::{DataType, Field, Schema};

use super::{
    CodecError, MAX_LIST_I32_VALUES_PER_ROW, MAX_LIST_I32_VALUES_PER_SECTION, read_list_i32,
    write_list_i32,
};

#[test]
fn list_i32_roundtrips() {
    let arr = write_list_i32(
        "blocked_by",
        vec![vec![1, 2, 3], vec![], vec![0, 7]].into_iter(),
    )
    .expect("write");
    let field = Field::new(
        "blocked_by",
        DataType::List(Arc::new(Field::new("item", DataType::Int32, false))),
        false,
    );
    let batch = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![arr]).expect("batch");
    let col = read_list_i32(&batch, "blocked_by").expect("read");
    assert_eq!(col.value(0), vec![1, 2, 3]);
    assert_eq!(col.value(1), Vec::<i32>::new());
    assert_eq!(col.value(2), vec![0, 7]);
}

#[test]
fn list_i32_rejects_null_list() {
    let arr = Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>([
        Some(vec![Some(1)]),
        None,
    ]));
    let field = Field::new(
        "blocked_by",
        DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
        true,
    );
    let batch = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![arr]).expect("batch");
    assert!(matches!(
        read_list_i32(&batch, "blocked_by"),
        Err(CodecError::NullInRequiredColumn { name: "blocked_by" })
    ));
}

#[test]
fn list_i32_rejects_null_child_value() {
    let arr = Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>([Some(
        vec![Some(1), None],
    )]));
    let field = Field::new(
        "blocked_by",
        DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
        false,
    );
    let batch = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![arr]).expect("batch");
    assert!(matches!(
        read_list_i32(&batch, "blocked_by"),
        Err(CodecError::NullInRequiredColumn { name: "blocked_by" })
    ));
}

#[test]
fn list_i32_rejects_oversized_row() {
    let err = write_list_i32(
        "blocked_by",
        [vec![0; MAX_LIST_I32_VALUES_PER_ROW + 1]].into_iter(),
    )
    .expect_err("oversized row rejected");
    assert!(matches!(
        err,
        CodecError::TooManyListValues {
            name: "blocked_by",
            values,
            max: MAX_LIST_I32_VALUES_PER_ROW
        } if values == MAX_LIST_I32_VALUES_PER_ROW + 1
    ));
}

#[test]
fn list_i32_rejects_oversized_section() {
    let row = vec![0; MAX_LIST_I32_VALUES_PER_ROW];
    let rows =
        (0..=(MAX_LIST_I32_VALUES_PER_SECTION / MAX_LIST_I32_VALUES_PER_ROW)).map(|_| row.clone());
    let err = write_list_i32("blocked_by", rows).expect_err("oversized section rejected");
    assert!(matches!(
        err,
        CodecError::TooManyListValues {
            name: "blocked_by",
            values,
            max: MAX_LIST_I32_VALUES_PER_SECTION
        } if values > MAX_LIST_I32_VALUES_PER_SECTION
    ));
}

#[test]
fn derive_list_i32_section_roundtrips() {
    use crate::Ts;

    #[derive(Debug, Clone, PartialEq, Eq, crate::Section)]
    #[section(id = 1_099_002, name = "list_probe", semantics = snapshot_full, sort_key("ts"))]
    struct Probe {
        #[column(t)]
        ts: Ts,
        #[column(l)]
        edges: Vec<i32>,
    }

    crate::assert_roundtrips(&[
        Probe {
            ts: Ts(10),
            edges: vec![1, 2],
        },
        Probe {
            ts: Ts(20),
            edges: vec![],
        },
    ]);
}
