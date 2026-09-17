use std::cell::Cell;

use kronika_query::snapshot::SnapshotPoint;

use super::{
    TimeSpecInput, resolve_bounded_range, resolve_point, resolve_range, resolve_range_with,
};

fn expression(value: &str) -> TimeSpecInput {
    TimeSpecInput::Expression(value.to_owned())
}

#[test]
fn resolves_integer_rfc3339_and_relative_forms() {
    let now = 1_800_000_000_000_000_i64;
    assert_eq!(
        TimeSpecInput::UnixMicros(i64::MIN).resolve(now),
        Ok(i64::MIN)
    );
    assert_eq!(
        TimeSpecInput::UnixMicros(i64::MAX).resolve(now),
        Ok(i64::MAX)
    );
    assert_eq!(expression("now").resolve(now), Ok(now));
    assert_eq!(expression("now-2us").resolve(now), Ok(now - 2));
    assert_eq!(expression("now-2ms").resolve(now), Ok(now - 2_000));
    assert_eq!(expression("now-2s").resolve(now), Ok(now - 2_000_000));
    assert_eq!(expression("now-2m").resolve(now), Ok(now - 120_000_000));
    assert_eq!(expression("now-2h").resolve(now), Ok(now - 7_200_000_000));
    assert_eq!(expression("now-2d").resolve(now), Ok(now - 172_800_000_000));
    assert_eq!(
        expression("now-2w").resolve(now),
        Ok(now - 1_209_600_000_000)
    );
    assert_eq!(
        expression("1970-01-01T00:00:01.123456Z").resolve(now),
        Ok(1_123_456)
    );
    assert_eq!(
        expression("1970-01-01T01:00:01+01:00").resolve(now),
        Ok(1_000_000)
    );
    assert_eq!(
        expression("1970-01-01t01:00:01+01:00").resolve(now),
        Ok(1_000_000)
    );
    assert_eq!(
        expression("1969-12-31T23:00:01-01:00").resolve(now),
        Ok(1_000_000)
    );
    assert_eq!(
        expression("1970-01-01T00:00:01.123456000Z").resolve(now),
        Ok(1_123_456)
    );
    assert_eq!(
        expression("1970-01-01T00:00:01.123456000000Z").resolve(now),
        Ok(1_123_456)
    );
    assert!(resolve_range(&TimeSpecInput::UnixMicros(1), &TimeSpecInput::UnixMicros(2)).is_ok());
}

#[test]
fn decimal_string_outputs_round_trip_as_time_inputs() {
    for timestamp in [i64::MIN, -1, 0, 1, i64::MAX] {
        let output = serde_json::Value::String(timestamp.to_string());
        let input: TimeSpecInput = serde_json::from_value(output).expect("time input");
        assert_eq!(input.resolve(7), Ok(timestamp));
    }
}

#[test]
fn rejects_every_unlisted_form_and_overflow() {
    for invalid in [
        "+1",
        "01",
        "-0",
        "-01",
        "9223372036854775808",
        "-9223372036854775809",
        "+",
        "-",
        "  now",
        "now ",
        "now-1month",
        "now-1y",
        "now--1s",
        "now-+1s",
        "now-1.5s",
        "2026-08-29",
        "2026-08-29T12:00:00",
        "1970-01-01t00:00:01z",
        "1970-01-01 00:00:01Z",
        "tomorrow",
        "1970-01-01T00:00:01.123456001Z",
        "1970-01-01T00:00:01.1234560001Z",
    ] {
        assert!(
            expression(invalid).resolve(0).is_err(),
            "accepted {invalid:?}"
        );
    }
    assert!(expression("now-9223372036854775807w").resolve(0).is_err());
    assert!(expression("now-1us").resolve(i64::MIN).is_err());
    assert!(serde_json::from_str::<TimeSpecInput>("9223372036854775808").is_err());
    assert!(serde_json::from_str::<TimeSpecInput>("-9223372036854775809").is_err());
    assert!(serde_json::from_str::<TimeSpecInput>("1.0").is_err());
    assert!(serde_json::from_str::<TimeSpecInput>("1e3").is_err());
}

#[test]
fn one_clock_anchor_resolves_both_bounds() {
    let calls = Cell::new(0_u8);
    let range = resolve_range_with(&expression("now-1h"), &expression("now"), || {
        calls.set(calls.get() + 1);
        Ok(9_000_000_000)
    })
    .expect("range");
    assert_eq!(calls.get(), 1);
    assert_eq!(range.from(), 5_400_000_000);
    assert_eq!(range.to_exclusive(), 9_000_000_000);
    assert!(resolve_range_with(&expression("now"), &expression("now-1us"), || Ok(7)).is_err());
    assert!(resolve_range_with(&expression("now"), &expression("now"), || Ok(7)).is_ok());

    assert!(
        resolve_bounded_range(
            &TimeSpecInput::UnixMicros(0),
            &TimeSpecInput::UnixMicros(11),
            10,
        )
        .is_err()
    );
    assert_eq!(
        resolve_bounded_range(
            &TimeSpecInput::UnixMicros(0),
            &TimeSpecInput::UnixMicros(10),
            10,
        )
        .expect("exact maximum width")
        .to_exclusive(),
        10
    );
    assert_eq!(resolve_point(None), Ok(SnapshotPoint::LatestRecorded));
    assert_eq!(
        resolve_point(Some(&TimeSpecInput::UnixMicros(42))),
        Ok(SnapshotPoint::At(42))
    );
}
