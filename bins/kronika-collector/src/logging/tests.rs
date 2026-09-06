use super::{LogLevel, field, parse_log_level_value, push_log_value, render_log_line};

#[test]
fn a_line_starts_with_the_binary_and_level() {
    let line = render_log_line(
        LogLevel::Info,
        "segment_written",
        &[field("bytes", 4_096_u64)],
    );
    assert_eq!(
        line,
        "kronika-collector level=info action=segment_written bytes=4096"
    );
}

#[test]
fn a_value_with_whitespace_or_quotes_is_quoted_and_escaped() {
    let mut out = String::new();
    push_log_value(&mut out, "/var/lib/kronika data");
    assert_eq!(out, "\"/var/lib/kronika data\"");

    let mut out = String::new();
    push_log_value(&mut out, "say \"hi\"");
    assert_eq!(out, "\"say \\\"hi\\\"\"");
}

#[test]
fn a_control_character_never_reaches_the_log_line_raw() {
    let mut out = String::new();
    push_log_value(&mut out, "bad\u{1b}value");
    assert_eq!(out, "\"bad\\u{1b}value\"");
}

#[test]
fn a_bare_word_needs_no_quotes() {
    let mut out = String::new();
    push_log_value(&mut out, "os_core");
    assert_eq!(out, "os_core");
}

#[test]
fn log_levels_parse_case_insensitively_and_reject_the_unknown() {
    assert_eq!(parse_log_level_value("debug"), Some(LogLevel::Debug));
    assert_eq!(parse_log_level_value("INFO"), Some(LogLevel::Info));
    assert_eq!(parse_log_level_value(" Warn "), Some(LogLevel::Warn));
    assert_eq!(parse_log_level_value("error"), Some(LogLevel::Error));
    assert_eq!(parse_log_level_value("chatty"), None);
    assert_eq!(parse_log_level_value(""), None);
}

#[test]
fn a_failure_line_carries_the_section_and_the_error() {
    let line = render_log_line(
        LogLevel::Warn,
        "collection_failed",
        &[
            field("collection", "os_meminfo"),
            field("type_id", 1_104_001_u64),
            field("error", "permission denied"),
        ],
    );
    assert_eq!(
        line,
        "kronika-collector level=warn action=collection_failed collection=os_meminfo \
         type_id=1104001 error=\"permission denied\""
    );
}
