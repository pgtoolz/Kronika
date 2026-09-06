use super::LinePrefix;
use crate::timestamp::local_micros;

#[test]
fn the_debian_default_names_the_user_and_the_database() {
    let prefix = LinePrefix::parse("%m [%p] %q%u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 MSK [12345] alice@shop ");

    assert_eq!(
        fields.ts,
        Some(local_micros("2026-08-07 12:34:56") + 789_000)
    );
    assert_eq!(fields.username, Some("alice".to_owned()));
    assert_eq!(fields.database, Some("shop".to_owned()));
}

#[test]
fn a_background_process_writes_nothing_after_the_session_marker() {
    let prefix = LinePrefix::parse("%m [%p] %q%u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 MSK [12345] ");

    assert_eq!(
        fields.ts,
        Some(local_micros("2026-08-07 12:34:56") + 789_000)
    );
    assert_eq!(fields.username, None);
    assert_eq!(fields.database, None);
}

#[test]
fn the_upstream_default_carries_only_the_time_and_the_process() {
    let prefix = LinePrefix::parse("%m [%p] ");

    let fields = prefix.read("2026-08-07 12:34:56.789 MSK [12345] ");

    assert_eq!(
        fields.ts,
        Some(local_micros("2026-08-07 12:34:56") + 789_000)
    );
    assert_eq!(fields.username, None);
}

#[test]
fn a_prefix_the_line_does_not_match_gives_up_where_it_stops() {
    let prefix = LinePrefix::parse("%m [%p] %u@%d ");

    let fields = prefix.read("2026-08-07 12:34:56.789 MSK ");

    assert!(fields.ts.is_some());
    assert_eq!(fields.username, None);
    assert_eq!(fields.database, None);
}

#[test]
fn a_percent_sign_in_the_prefix_is_matched_as_one() {
    let prefix = LinePrefix::parse("%% %d ");

    let fields = prefix.read("% shop ");

    assert_eq!(fields.database, Some("shop".to_owned()));
}
