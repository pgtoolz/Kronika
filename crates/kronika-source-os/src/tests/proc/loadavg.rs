use super::parse_loadavg;

const SAMPLE: &str = "0.15 0.10 0.05 2/345 6789\n";

#[test]
fn parses_valid_line() {
    let row = parse_loadavg(SAMPLE, 9_999).expect("parse");
    assert_eq!(row.ts, 9_999);
    assert!((row.load1 - 0.15).abs() < 1e-9);
    assert!((row.load5 - 0.10).abs() < 1e-9);
    assert!((row.load15 - 0.05).abs() < 1e-9);
    assert_eq!(row.running, 2);
    assert_eq!(row.total, 345);
}

#[test]
fn malformed_run_total_token_is_an_error() {
    assert!(parse_loadavg("0.15 0.10 0.05 bad 6789\n", 1).is_err());
}

#[test]
fn non_numeric_load_is_an_error() {
    assert!(parse_loadavg("abc 0.10 0.05 2/345 6789\n", 1).is_err());
}

#[test]
fn empty_content_is_an_error() {
    assert!(parse_loadavg("", 1).is_err());
}

#[test]
fn to_section_carries_every_floor_field_and_scope() {
    let section = parse_loadavg(SAMPLE, 9_999).expect("parse").to_section(0);
    assert_eq!(section.ts.0, 9_999);
    assert!((section.load1 - 0.15).abs() < 1e-9);
    assert!((section.load5 - 0.10).abs() < 1e-9);
    assert!((section.load15 - 0.05).abs() < 1e-9);
    assert_eq!(section.running, 2);
    assert_eq!(section.total, 345);
    assert_eq!(section.scope, 0);
}
