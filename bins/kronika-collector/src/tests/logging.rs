use super::{LogLevel, field, render_log_line};

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
    let line = render_log_line(
        LogLevel::Info,
        "escaping",
        &[
            field("path", "/var/lib/kronika data"),
            field("message", "say \"hi\""),
        ],
    );
    assert_eq!(
        line,
        r#"kronika-collector level=info action=escaping path="/var/lib/kronika data" message="say \"hi\"""#
    );
}

#[test]
fn a_control_character_never_reaches_the_log_line_raw() {
    let line = render_log_line(
        LogLevel::Info,
        "escaping",
        &[field("value", "bad\u{1b}value")],
    );
    assert_eq!(
        line,
        r#"kronika-collector level=info action=escaping value="bad\u{1b}value""#
    );
}

#[test]
fn a_bare_word_needs_no_quotes() {
    let line = render_log_line(LogLevel::Info, "escaping", &[field("value", "os_core")]);
    assert_eq!(
        line,
        "kronika-collector level=info action=escaping value=os_core"
    );
}

#[test]
fn log_levels_parse_case_insensitively_and_reject_the_unknown() {
    assert_eq!(LogLevel::parse("debug"), Some(LogLevel::Debug));
    assert_eq!(LogLevel::parse("INFO"), Some(LogLevel::Info));
    assert_eq!(LogLevel::parse(" Warn "), Some(LogLevel::Warn));
    assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
    assert_eq!(LogLevel::parse("chatty"), None);
    assert_eq!(LogLevel::parse(""), None);
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

#[test]
fn field_values_preserve_scalar_ranges_and_display_text() {
    let address = std::net::Ipv4Addr::LOCALHOST;
    let display: &dyn std::fmt::Display = &address;
    let line = render_log_line(
        LogLevel::Info,
        "field_types",
        &[
            field("i32", i32::MIN),
            field("i64", i64::MIN),
            field("u32", u32::MAX),
            field("u64", u64::MAX),
            field("u128", u128::MAX),
            field("usize", 42_usize),
            field("yes", true),
            field("no", false),
            field("some", Some(u64::MAX)),
            field("none", None::<u64>),
            field("owned", "owned value".to_owned()),
            field("borrowed", "строка"),
            field("path", std::path::Path::new("/tmp/log files").display()),
            field("display", display),
        ],
    );

    assert_eq!(
        line,
        "kronika-collector level=info action=field_types \
         i32=-2147483648 i64=-9223372036854775808 u32=4294967295 \
         u64=18446744073709551615 u128=340282366920938463463374607431768211455 \
         usize=42 yes=true no=false some=18446744073709551615 none=unavailable \
         owned=\"owned value\" borrowed=строка path=\"/tmp/log files\" display=127.0.0.1"
    );
}

#[test]
fn borrowed_display_is_formatted_once_when_the_line_is_rendered() {
    struct CountedDisplay<'a>(&'a std::cell::Cell<usize>);

    impl std::fmt::Display for CountedDisplay<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.set(self.0.get() + 1);
            f.write_str("bad=\"\\\n\r\t\u{1b}данные")
        }
    }

    let calls = std::cell::Cell::new(0);
    let value = CountedDisplay(&calls);
    let fields = [field("value", &value)];
    assert_eq!(calls.get(), 0, "constructing a field must not format it");

    let line = render_log_line(LogLevel::Warn, "deferred", &fields);

    assert_eq!(calls.get(), 1, "render each Display exactly once");
    assert_eq!(
        line,
        r#"kronika-collector level=warn action=deferred value="bad=\"\\\n\r\t\u{1b}данные""#
    );
}

#[test]
fn string_fields_escape_logfmt_delimiters_without_changing_unicode() {
    for (value, encoded) in [
        ("", r#""""#),
        ("=", r#""=""#),
        ("\\", r#""\\""#),
        ("\n\r\t", r#""\n\r\t""#),
        ("ёж", "ёж"),
        ("a\u{a0}b", "\"a\u{a0}b\""),
    ] {
        assert_eq!(
            render_log_line(LogLevel::Info, "escaping", &[field("value", value)]),
            format!("kronika-collector level=info action=escaping value={encoded}"),
            "input {value:?}"
        );
    }
}
