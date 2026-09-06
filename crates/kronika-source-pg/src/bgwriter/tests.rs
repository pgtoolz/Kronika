use super::{BgwriterVersion, bgwriter_query, bgwriter_version};

#[test]
fn version_changes_when_postgresql_splits_the_view() {
    assert_eq!(bgwriter_version(10), BgwriterVersion::V1);
    assert_eq!(bgwriter_version(16), BgwriterVersion::V1);
    assert_eq!(bgwriter_version(17), BgwriterVersion::V2);
    assert_eq!(bgwriter_version(18), BgwriterVersion::V2);
}

#[test]
fn queries_use_exact_version_columns_and_a_marker() {
    let old = bgwriter_query(BgwriterVersion::V1);
    let new = bgwriter_query(BgwriterVersion::V2);
    assert!(old.contains("checkpoints_timed"));
    assert!(old.contains("buffers_backend_fsync"));
    assert!(!new.contains("checkpoints_timed"));
    assert!(!new.contains("buffers_backend"));
    assert!(new.contains("buffers_clean"));
    assert!(old.contains("kronika:"));
    assert!(new.contains("kronika:"));
}
