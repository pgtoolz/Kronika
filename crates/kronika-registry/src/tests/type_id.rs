use super::{SectionClass, TypeId};

#[test]
fn decomposes_the_digits() {
    let id = TypeId::new(1_006_001).expect("valid");
    assert_eq!(id.class_digit(), 1);
    assert_eq!(id.source(), 6);
    assert_eq!(id.version(), 1);
    assert_eq!(id.section_class(), Some(SectionClass::Snapshot));
    assert_eq!(id.get(), 1_006_001);
}

#[test]
fn charts_use_the_two_digit_class() {
    let id = TypeId::new(10_001_001).expect("valid chart id");
    assert_eq!(id.class_digit(), 10);
    assert_eq!(id.section_class(), Some(SectionClass::Chart));
    assert_eq!(id.source(), 1);
    assert_eq!(id.version(), 1);
}

#[test]
fn rejects_unknown_class_zero_source_and_zero_version() {
    // Class 4 is not assigned.
    assert_eq!(TypeId::new(4_000_001), None);
    // Source must start at 1.
    assert_eq!(TypeId::new(1_000_001), None);
    // Version must start at 1.
    assert_eq!(TypeId::new(1_006_000), None);
}
