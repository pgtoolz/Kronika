use super::{CheckpointerVersion, checkpointer_query, checkpointer_version};

#[test]
fn view_appears_at_pg17_and_changes_at_pg18() {
    assert_eq!(checkpointer_version(16), None);
    assert_eq!(checkpointer_version(17), Some(CheckpointerVersion::V1));
    assert_eq!(checkpointer_version(18), Some(CheckpointerVersion::V2));
}

#[test]
fn queries_use_exact_version_columns() {
    let v17 = checkpointer_query(CheckpointerVersion::V1);
    let v18 = checkpointer_query(CheckpointerVersion::V2);
    assert!(!v17.contains("num_done"));
    assert!(!v17.contains("slru_written"));
    assert!(v18.contains("num_done"));
    assert!(v18.contains("slru_written"));
    assert!(v17.contains("kronika:"));
    assert!(v18.contains("kronika:"));
}
