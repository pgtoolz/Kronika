use super::LinePrefix;
use crate::timestamp::utc_micros;

#[test]
fn the_debian_default_names_the_user_and_the_database() {
    let prefix = LinePrefix::parse("%m [%p] %q%u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 UTC [12345] alice@shop ", None);

    assert_eq!(fields.ts, Some(utc_micros("2026-08-07 12:34:56") + 789_000));
    assert_eq!(fields.username, Some("alice".to_owned()));
    assert_eq!(fields.database, Some("shop".to_owned()));
}

#[test]
fn a_background_process_writes_nothing_after_the_session_marker() {
    let prefix = LinePrefix::parse("%m [%p] %q%u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 UTC [12345] ", None);

    assert_eq!(fields.ts, Some(utc_micros("2026-08-07 12:34:56") + 789_000));
    assert_eq!(fields.username, None);
    assert_eq!(fields.database, None);
}

#[test]
fn the_upstream_default_carries_only_the_time_and_the_process() {
    let prefix = LinePrefix::parse("%m [%p] ");

    let fields = prefix.read("2026-08-07 12:34:56.789 UTC [12345] ", None);

    assert_eq!(fields.ts, Some(utc_micros("2026-08-07 12:34:56") + 789_000));
    assert_eq!(fields.username, None);
}

#[test]
fn a_prefix_the_line_does_not_match_gives_up_where_it_stops() {
    let prefix = LinePrefix::parse("%m [%p] %u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 UTC ", None);

    assert!(fields.ts.is_some());
    assert_eq!(fields.username, None);
    assert_eq!(fields.database, None);
}

#[test]
fn a_percent_sign_in_the_prefix_is_matched_as_one() {
    let prefix = LinePrefix::parse("%% %d ");

    let fields = prefix.read("% shop ", None);

    assert_eq!(fields.database, Some("shop".to_owned()));
}

#[test]
fn event_time_priority_does_not_depend_on_prefix_order() {
    let instant = 1_789_380_780_789_000;
    for (setting, head) in [
        (
            "%n %m %t %s ",
            "1789380780.789 2026-09-14 10:13:00.789 GMT 2026-09-14 10:13:01 GMT 2026-09-10 01:00:00 GMT ",
        ),
        (
            "%s %t %m %n ",
            "2026-09-10 01:00:00 GMT 2026-09-14 10:13:01 GMT 2026-09-14 10:13:00.789 GMT 1789380780.789 ",
        ),
        (
            "%m %t ",
            "2026-09-14 10:13:00.789 GMT 2026-09-14 10:13:00 GMT ",
        ),
        ("%m %n ", "2026-09-14 10:13:00.789 UNKNOWN 1789380780.789 "),
        ("%n %m ", "1789380780.789 2026-09-14 10:13:00.789 UNKNOWN "),
    ] {
        assert_eq!(
            LinePrefix::parse(setting).read(head, None).ts,
            Some(instant),
            "{setting}"
        );
    }
    let prefix = LinePrefix::parse("%s %u@%d ");
    let fields = prefix.read("2026-09-10 01:00:00 GMT alice@shop ", None);
    assert_eq!(fields.ts, None);
    assert_eq!(fields.database.as_deref(), Some("shop"));
    assert!(!prefix.has_event_time());
}

#[test]
fn a_session_marker_does_not_discard_time_before_it() {
    let fields = LinePrefix::parse("%m [%p]%q %n ").read("2026-09-14 10:13:00.789 GMT [123]", None);
    assert!(fields.time_expected);
    assert_eq!(fields.ts, Some(1_789_380_780_789_000));
}

#[test]
fn a_colon_literal_ends_an_abbreviated_timezone_before_the_next_field() {
    for (setting, head, expected) in [
        (
            "%t: [%p] ",
            "2026-09-14 10:13:00 GMT: [1] ",
            1_789_380_780_000_000,
        ),
        (
            "%t:%n ",
            "2026-09-14 10:13:00 GMT:1789380780.789 ",
            1_789_380_780_789_000,
        ),
        (
            "%t [%p] ",
            "2026-09-14 15:43:00 +05:30 [1] ",
            1_789_380_780_000_000,
        ),
        (
            "%t: [%p] ",
            "2026-09-14 13:13:00 +03: [1] ",
            1_789_380_780_000_000,
        ),
        (
            "%t:%n ",
            "2026-09-14 13:13:00 +03:1789380780.789 ",
            1_789_380_780_789_000,
        ),
        (
            "%t: [%p] ",
            "2026-09-14 15:43:00 +05:30: [1] ",
            1_789_380_780_000_000,
        ),
        (
            "%t:%n ",
            "2026-09-14 15:43:00 +05:30:1789380780.789 ",
            1_789_380_780_789_000,
        ),
    ] {
        assert_eq!(
            LinePrefix::parse(setting).read(head, None).ts,
            Some(expected)
        );
    }
}

#[test]
fn a_numeric_timezone_label_does_not_consume_a_two_digit_pid() {
    let zone = crate::timestamp::LogTimezone::parse("Etc/GMT-3").expect("zone");
    for (setting, head) in [
        ("%t:%p ", "2026-09-14 13:13:00 +03:34 "),
        ("%t:%p:%d ", "2026-09-14 13:13:00 +03:34:shop "),
    ] {
        let fields = LinePrefix::parse(setting).read(head, Some(&zone));
        assert_eq!(fields.ts, Some(1_789_380_780_000_000));
        if setting.contains("%d") {
            assert_eq!(fields.database.as_deref(), Some("shop"));
        }
    }
    let fields = LinePrefix::parse("%t:%p ").read("2026-09-14 15:43:00 +05:30:34 ", None);
    assert_eq!(fields.ts, Some(1_789_380_780_000_000));
}

#[test]
fn repeated_literals_cannot_turn_a_pid_into_offset_minutes_or_seconds() {
    for (head, expected) in [
        ("2026-09-14 13:13:00 +03:34:shop ", 1_789_380_780_000_000),
        ("2026-09-14 15:43:00 +05:30:34:shop ", 1_789_380_780_000_000),
        (
            "2026-09-14 15:43:34 +05:30:34:123:shop ",
            1_789_380_780_000_000,
        ),
    ] {
        let fields = LinePrefix::parse("%t:%p:%d ").read(head, None);
        assert_eq!(fields.ts, Some(expected), "{head}");
        assert_eq!(fields.database.as_deref(), Some("shop"), "{head}");
    }
}

#[test]
fn numeric_fields_and_later_timestamps_use_their_own_boundaries() {
    let zone = crate::timestamp::LogTimezone::parse("Etc/GMT-3").expect("zone");
    let fields = LinePrefix::parse("%t:%p:%l ").read("2026-09-14 13:13:00 +03:12:55 ", Some(&zone));
    assert_eq!(fields.ts, Some(1_789_380_780_000_000));
    for zone in [None, Some(&zone)] {
        let fields = LinePrefix::parse("%s:%p %m:%p ").read(
            "2026-09-01 13:13:00 +03:34 2026-09-14 13:13:00.789 +03:34 ",
            zone,
        );
        assert_eq!(fields.ts, Some(1_789_380_780_789_000));
    }
}
