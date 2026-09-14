use super::{LogTimezone, epoch, parse, parse_local, utc_micros};

#[test]
fn explicit_zones_do_not_use_the_host_clock() {
    let expected = 1_789_380_780_789_012;
    for (clock, label) in [
        ("10:13:00", "GMT"),
        ("10:13:00", "UTC"),
        ("10:13:00", "Z"),
        ("15:43:00", "+05:30"),
        ("16:58:00", "+06:45"),
        ("06:13:00", "-04"),
    ] {
        let input = format!("2026-09-14 {clock}.789012 {label} [123]");
        assert_eq!(parse(&input, None), Ok((expected, " [123]")), "{input}");
        assert_eq!(parse_local(&input), Some((expected, " [123]")));
    }
}

#[test]
fn configured_zone_resolves_dst_and_validates_the_printed_label() {
    let zone = LogTimezone::parse("America/New_York").expect("zone");
    for (wall, label, utc) in [
        ("2026-09-14 06:13:00", "EDT", "2026-09-14 10:13:00"),
        ("2026-01-14 05:13:00", "EST", "2026-01-14 10:13:00"),
        ("2026-11-01 01:30:00", "EDT", "2026-11-01 05:30:00"),
        ("2026-11-01 01:30:00", "EST", "2026-11-01 06:30:00"),
    ] {
        assert_eq!(
            parse(&format!("{wall} {label}"), Some(&zone)).map(|(ts, _)| ts),
            Ok(utc_micros(utc))
        );
    }
    for value in [
        "2026-03-08 02:30:00 EST",
        "2026-11-01 01:30:00",
        "2026-09-14 10:13:00 GMT",
    ] {
        assert!(parse(value, Some(&zone)).is_err(), "{value}");
    }
}

#[test]
fn postgres_posix_labels_do_not_override_the_configured_offset() {
    for (setting, label) in [("GMT+4", "GMT"), ("<+03>4", "+03")] {
        let zone = LogTimezone::parse(setting).expect("POSIX zone");
        assert_eq!(
            parse(&format!("2026-09-14 06:13:00 {label}"), Some(&zone)).map(|(ts, _)| ts),
            Ok(1_789_380_780_000_000)
        );
    }
}

#[test]
fn malformed_or_unknown_timestamps_are_not_observation_time() {
    for value in [
        "",
        "2026-09-14T10:13:00 UTC",
        "2026-13-14 10:13:00 UTC",
        "2026-09-14 25:13:00 UTC",
        "2026-09-14 10:13:00. UTC",
        "2026-09-14 10:13:00 UNKNOWN",
        "2026-09-14 10:13:00 EST",
        "2026-09-14 10:13:00 CST",
        "2026-09-14 10:13:00",
        "2026-02-29 10:13:00 GMT",
    ] {
        assert!(parse(value, None).is_err(), "{value}");
    }
    assert!(parse("2024-02-29 10:13:00 GMT", None).is_ok());
    assert!(parse("2026-03-08 02:30:00 GMT", None).is_ok());
}

#[test]
fn epoch_is_seconds_and_fraction_without_floating_point() {
    assert_eq!(
        epoch("1789380780.789 [1]"),
        Some((1_789_380_780_789_000, " [1]"))
    );
    assert_eq!(epoch("-0.125 "), Some((-125_000, " ")));
    for value in ["1.", "9223372036854775807.123", "--1.2"] {
        assert!(epoch(value).is_none());
    }
}

#[test]
fn pgbouncer_without_a_zone_keeps_the_local_contract_and_tail() {
    let (ts, tail) = parse_local("2026-09-14 10:13:00.123 [1]").expect("local clock");
    assert_eq!(tail, " [1]");
    assert_eq!(ts % 1_000_000, 123_000);
}

#[test]
fn pgbouncer_local_abbreviations_keep_the_writers_clock() {
    const CHILD: &str = "KRONIKA_TEST_PGBOUNCER_LOCAL_ZONE";
    if let Ok(line) = std::env::var(CHILD) {
        assert_eq!(
            parse_local(&line).map(|(ts, _)| ts),
            Some(1_789_380_780_000_000)
        );
        return;
    }
    for (zone, line) in [
        ("America/New_York", "2026-09-14 06:13:00 EDT [1]"),
        ("Europe/Moscow", "2026-09-14 13:13:00 MSK [1]"),
    ] {
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "timestamp::tests::pgbouncer_local_abbreviations_keep_the_writers_clock",
            ])
            .env("TZ", zone)
            .env(CHILD, line)
            .output()
            .expect("child");
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stdout)
        );
    }
}
