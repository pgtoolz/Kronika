use super::{local_micros, parse_local};

#[test]
fn reads_the_wall_clock_in_the_host_timezone() {
    let (ts, rest) = parse_local("2026-08-07 12:34:56 MSK [12345]").expect("parsed");

    assert_eq!(ts, local_micros("2026-08-07 12:34:56"));
    assert_eq!(rest, " [12345]");
}

#[test]
fn keeps_the_milliseconds_pgbouncer_prints() {
    let (ts, _rest) = parse_local("2026-08-07 12:34:56.789 MSK").expect("parsed");

    assert_eq!(ts, local_micros("2026-08-07 12:34:56") + 789_000);
}

#[test]
fn keeps_the_microseconds_postgresql_prints() {
    let (ts, _rest) = parse_local("2026-08-07 12:34:56.789012 UTC").expect("parsed");

    assert_eq!(ts, local_micros("2026-08-07 12:34:56") + 789_012);
}

#[test]
fn a_timestamp_with_no_zone_ends_where_it_ends() {
    let (_ts, rest) = parse_local("2026-08-07 12:34:56 [12345]").expect("parsed");

    assert_eq!(rest, " [12345]");
}

#[test]
fn text_that_is_not_a_timestamp_is_not_read_as_one() {
    for line in [
        "",
        "LOG:  checkpoint starting",
        "2026-08-07T12:34:56 MSK",
        "2026-13-07 12:34:56 MSK",
        "2026-08-07 25:00:00 MSK",
    ] {
        assert_eq!(parse_local(line), None, "{line:?}");
    }
}
