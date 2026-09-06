use super::CodecError;

#[test]
fn section_type_id_labels_the_two_type_tagged_outcomes_and_nothing_else() {
    assert_eq!(
        CodecError::UnknownType { type_id: 5 }.section_type_id(),
        Some(5)
    );
    let wrapped = CodecError::Section {
        type_id: 7,
        bytes_in: 64,
        source: Box::new(CodecError::SchemaMismatch),
    };
    assert_eq!(wrapped.section_type_id(), Some(7));
    assert_eq!(CodecError::SchemaMismatch.section_type_id(), None);
    assert_eq!(
        CodecError::TooManyRows { rows: 9, max: 8 }.section_type_id(),
        None,
        "errors not tied to one section have no label"
    );
}

#[test]
fn required_column_rejects_a_null_so_it_cannot_read_as_zero() {
    use std::sync::Arc;

    use arrow_array::types::Int64Type;
    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};

    use super::required_column;

    // Required columns must not decode NULL as zero.
    let schema = Arc::new(Schema::new(vec![Field::new("ts", DataType::Int64, true)]));
    let column: ArrayRef = Arc::new(Int64Array::from(vec![Some(1), None]));
    let batch = RecordBatch::try_new(schema, vec![column]).expect("batch");
    assert!(matches!(
        required_column::<Int64Type>(&batch, "ts"),
        Err(CodecError::NullInRequiredColumn { name: "ts" })
    ));
}
