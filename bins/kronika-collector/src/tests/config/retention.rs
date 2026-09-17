use super::RetentionConfig;

#[test]
fn a_bare_byte_budget_is_a_fixed_target() {
    assert_eq!(
        RetentionConfig::parse("1073741824").expect("a byte budget"),
        RetentionConfig::Fixed(1_073_741_824)
    );
}

#[test]
fn auto_without_a_suffix_uses_the_default_percentage() {
    assert_eq!(
        RetentionConfig::parse("auto").expect("auto"),
        RetentionConfig::Auto(80)
    );
    assert_eq!(
        RetentionConfig::parse(" auto:55 ").expect("auto with a percentage"),
        RetentionConfig::Auto(55)
    );
}

#[test]
fn an_out_of_range_or_malformed_target_is_rejected() {
    assert!(RetentionConfig::parse("").is_err());
    assert!(RetentionConfig::parse("auto:0").is_err());
    assert!(RetentionConfig::parse("auto:100").is_err());
    assert!(RetentionConfig::parse("auto-80").is_err());
    assert!(RetentionConfig::parse("plenty").is_err());
}

#[test]
fn a_fixed_budget_below_two_segments_cannot_converge() {
    let segment = 64 * 1024 * 1024;
    assert!(
        RetentionConfig::Fixed(2 * segment)
            .validate(segment)
            .is_ok()
    );
    assert!(
        RetentionConfig::Fixed(2 * segment - 1)
            .validate(segment)
            .is_err()
    );
    // `auto` targets a live partition fraction and has no such floor.
    assert!(RetentionConfig::Auto(1).validate(segment).is_ok());
}
