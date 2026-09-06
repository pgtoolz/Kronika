use kronika_reader::SegmentKind;

use super::validate_checksum;

#[test]
fn active_index_rejects_a_reusable_checksum() {
    assert!(validate_checksum(SegmentKind::Active, None).is_ok());
    let error = validate_checksum(SegmentKind::Active, Some(7))
        .expect_err("active index checksum must be rejected");
    assert_eq!(
        error.to_string(),
        "active index unexpectedly carries a reusable checksum"
    );
}
