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
