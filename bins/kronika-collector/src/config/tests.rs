use super::{
    RetentionConfig, parse_retention, validate_journal_max_bytes, validate_retention,
    validate_segment_max_bytes,
};
use kronika_format::{JOURNAL_HEADER_LEN, MAX_JOURNAL_LEN};

#[test]
fn a_bare_byte_budget_is_a_fixed_target() {
    assert_eq!(
        parse_retention("1073741824").expect("a byte budget"),
        RetentionConfig::Fixed(1_073_741_824)
    );
}

#[test]
fn auto_without_a_suffix_uses_the_default_percentage() {
    assert_eq!(
        parse_retention("auto").expect("auto"),
        RetentionConfig::Auto(80)
    );
    assert_eq!(
        parse_retention(" auto:55 ").expect("auto with a percentage"),
        RetentionConfig::Auto(55)
    );
}

#[test]
fn an_out_of_range_or_malformed_target_is_rejected() {
    assert!(parse_retention("").is_err());
    assert!(parse_retention("auto:0").is_err());
    assert!(parse_retention("auto:100").is_err());
    assert!(parse_retention("auto-80").is_err());
    assert!(parse_retention("plenty").is_err());
}

#[test]
fn a_fixed_budget_below_two_segments_cannot_converge() {
    let segment = 64 * 1024 * 1024;
    assert!(validate_retention(RetentionConfig::Fixed(2 * segment), segment).is_ok());
    assert!(validate_retention(RetentionConfig::Fixed(2 * segment - 1), segment).is_err());
    // `auto` targets a live partition fraction and has no such floor.
    assert!(validate_retention(RetentionConfig::Auto(1), segment).is_ok());
}

#[test]
fn the_journal_cap_must_fit_the_format() {
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64).is_ok());
    assert!(validate_journal_max_bytes(JOURNAL_HEADER_LEN as u64 - 1).is_err());
    assert!(validate_journal_max_bytes(MAX_JOURNAL_LEN as u64 + 1).is_err());
}

#[test]
fn the_segment_cap_must_be_positive() {
    assert!(validate_segment_max_bytes(1).is_ok());
    assert!(validate_segment_max_bytes(0).is_err());
}

#[test]
fn a_value_that_is_not_a_number_names_itself_in_the_refusal() {
    let error =
        super::parse_env_number::<u64>("KRONIKA_INTERVAL_S", "often").expect_err("a refusal");
    assert_eq!(
        error.to_string(),
        r#"KRONIKA_INTERVAL_S="often" is not a whole number"#
    );
}

#[test]
fn a_negative_count_is_not_a_whole_number() {
    assert!(super::parse_env_number::<u64>("KRONIKA_SEGMENT_MAX_AGE_S", "-1").is_err());
}

#[test]
fn surrounding_whitespace_is_not_a_refusal() {
    assert_eq!(
        super::parse_env_number::<u64>("KRONIKA_INTERVAL_S", " 30 ").expect("a number"),
        30
    );
}

#[test]
fn an_empty_element_in_a_list_is_a_refusal_naming_the_variable() {
    let error = super::parse_env_list("KRONIKA_PG_LOGS", "/var/log/a.log;;/var/log/b.log")
        .expect_err("a refusal");

    assert_eq!(error.to_string(), "KRONIKA_PG_LOGS has an empty element");
}

#[test]
fn a_list_is_split_on_semicolons_and_trimmed() {
    let entries = super::parse_env_list("KRONIKA_PG_LOGS", " /var/log/a.log ; /var/log/b.log ")
        .expect("a list");

    assert_eq!(entries, ["/var/log/a.log", "/var/log/b.log"]);
}

#[test]
fn a_blank_list_is_empty_rather_than_one_blank_element() {
    assert!(
        super::parse_env_list("KRONIKA_PGBOUNCER_LOGS", "   ")
            .expect("a list")
            .is_empty()
    );
}

#[test]
fn postgres_capacity_is_positive_or_unknown() {
    assert_eq!(
        super::optional_positive_u32("KRONIKA_POSTGRES_EFFECTIVE_CPUS", None)
            .expect("unset capacity"),
        None
    );
    assert_eq!(
        super::optional_positive_u32("KRONIKA_POSTGRES_EFFECTIVE_CPUS", Some(" 2 "))
            .expect("positive capacity"),
        Some(2)
    );
    assert!(super::optional_positive_u32("KRONIKA_POSTGRES_EFFECTIVE_CPUS", Some("0")).is_err());
    assert!(super::optional_positive_u32("KRONIKA_POSTGRES_EFFECTIVE_CPUS", Some("two")).is_err());
}
