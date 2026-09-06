//! Reading the wall clock a log line was written at.

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone as _};

/// Parse `YYYY-MM-DD HH:MM:SS[.fff]` at the start of `text`.
///
/// Returns the unix microseconds and what follows the timestamp. Both
/// `PostgreSQL` and `PgBouncer` print the local wall clock and an abbreviated
/// zone name, so the clock is read in the host's timezone and the abbreviation
/// is skipped. A `log_timezone` that differs from the host's timezone shifts
/// the result by the difference between them.
#[must_use]
pub(crate) fn parse_local(text: &str) -> Option<(i64, &str)> {
    let date = text.get(..10)?;
    let clock = text.get(11..19)?;
    if text.as_bytes().get(10) != Some(&b' ') {
        return None;
    }
    let naive = parse_naive(date, clock)?;
    let mut rest = text.get(19..)?;
    let micros = if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(fraction.len());
        rest = fraction.get(digits..)?;
        fractional_micros(fraction.get(..digits)?)
    } else {
        0
    };
    let unix = Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|at| at.timestamp())?;
    Some((
        unix.checked_mul(1_000_000)?.checked_add(micros)?,
        skip_zone(rest),
    ))
}

#[cfg(test)]
pub(crate) fn local_micros(text: &str) -> i64 {
    let naive =
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").expect("a readable wall clock");
    Local
        .from_local_datetime(&naive)
        .earliest()
        .expect("the host timezone maps this reading")
        .timestamp()
        * 1_000_000
}

fn parse_naive(date: &str, clock: &str) -> Option<NaiveDateTime> {
    let year = date.get(..4)?.parse().ok()?;
    let month = date.get(5..7)?.parse().ok()?;
    let day = date.get(8..10)?.parse().ok()?;
    let hour = clock.get(..2)?.parse().ok()?;
    let minute = clock.get(3..5)?.parse().ok()?;
    let second = clock.get(6..8)?.parse().ok()?;
    if date.as_bytes().get(4) != Some(&b'-') || date.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    if clock.as_bytes().get(2) != Some(&b':') || clock.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)
}

/// Scale the fractional digits to microseconds, keeping the first six.
fn fractional_micros(digits: &str) -> i64 {
    let mut micros = 0_i64;
    let mut scale = 100_000_i64;
    for byte in digits.bytes().take(6) {
        micros += i64::from(byte - b'0') * scale;
        scale /= 10;
    }
    micros
}

/// Skip the ` MSK`-style zone abbreviation the timestamp is followed by.
fn skip_zone(rest: &str) -> &str {
    let Some(tail) = rest.strip_prefix(' ') else {
        return rest;
    };
    let end = tail
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-'))
        .unwrap_or(tail.len());
    if end == 0 {
        return rest;
    }
    tail.get(end..).unwrap_or_default()
}

#[cfg(test)]
mod tests;
