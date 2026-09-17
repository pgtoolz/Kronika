//! Resolving log wall clocks in the writer's timezone.

use chrono::{Datelike as _, Local, NaiveDate, NaiveDateTime, TimeZone as _, Timelike as _};
use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, Offset, TimeZone, TimeZoneDatabase};

/// The `PostgreSQL` server's `log_timezone` setting.
#[derive(Debug, Clone)]
pub struct LogTimezone(TimeZone);

impl LogTimezone {
    /// Resolve an IANA name or `PostgreSQL` POSIX timezone specification.
    ///
    /// # Errors
    /// Returns an error when the timezone is unknown or invalid.
    pub fn parse(name: &str) -> Result<Self, jiff::Error> {
        TimeZoneDatabase::bundled()
            .get(name)
            .or_else(|_| TimeZone::posix(name))
            .map(Self)
    }
}

pub(crate) const INVALID: &str =
    "PostgreSQL log timestamp is invalid or its timezone cannot be resolved";

pub(crate) fn parse<'a>(
    text: &'a str,
    zone: Option<&LogTimezone>,
) -> Result<(i64, &'a str), &'static str> {
    let (naive, micros, label, rest) = calendar(text).ok_or(INVALID)?;
    let dt = DateTime::new(
        naive.year().try_into().map_err(|_error| INVALID)?,
        naive.month().try_into().map_err(|_error| INVALID)?,
        naive.day().try_into().map_err(|_error| INVALID)?,
        naive.hour().try_into().map_err(|_error| INVALID)?,
        naive.minute().try_into().map_err(|_error| INVALID)?,
        naive.second().try_into().map_err(|_error| INVALID)?,
        0,
    )
    .map_err(|_error| INVALID)?;
    let instant = if let Some(zone) = zone {
        let ambiguous = zone.0.to_ambiguous_timestamp(dt);
        let offsets = match ambiguous.offset() {
            AmbiguousOffset::Unambiguous { offset } => [Some(offset), None],
            AmbiguousOffset::Fold { before, after } => [Some(before), Some(after)],
            AmbiguousOffset::Gap { .. } => return Err(INVALID),
        };
        let mut found = None;
        for offset in offsets.into_iter().flatten() {
            let at = offset.to_timestamp(dt).map_err(|_error| INVALID)?;
            let info = zone.0.to_offset_info(at);
            if label.is_empty()
                || label == info.abbreviation()
                || (offset == Offset::UTC && matches!(label, "UTC" | "GMT" | "Z"))
            {
                if found.is_some() {
                    return Err(INVALID);
                }
                found = Some(at);
            }
        }
        found.ok_or(INVALID)?
    } else {
        let zone = if matches!(label, "UTC" | "GMT" | "Z") {
            TimeZone::UTC
        } else {
            if !label.starts_with(['+', '-']) && !label.contains('/') {
                return Err(INVALID);
            }
            jiff::fmt::temporal::DateTimeParser::new()
                .parse_time_zone_with(&TimeZoneDatabase::bundled(), label)
                .map_err(|_error| INVALID)?
        };
        zone.to_ambiguous_timestamp(dt)
            .unambiguous()
            .map_err(|_error| INVALID)?
    };
    let ts = instant
        .as_microsecond()
        .checked_add(micros)
        .ok_or(INVALID)?;
    Ok((ts, rest))
}

/// `PgBouncer`'s unzoned wall clock belongs to the local process.
pub(crate) fn parse_local(text: &str) -> Option<(i64, &str)> {
    let (naive, micros, label, rest) = calendar(text)?;
    if matches!(label, "UTC" | "GMT" | "Z") || label.starts_with(['+', '-']) || label.contains('/')
    {
        return parse(text, None).ok();
    }
    let at = Local.from_local_datetime(&naive).earliest()?;
    Some((
        at.timestamp().checked_mul(1_000_000)?.checked_add(micros)?,
        rest,
    ))
}

pub(crate) fn epoch(text: &str) -> Option<(i64, &str)> {
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(text.len());
    let value = text.get(..end)?;
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i64 = seconds.parse().ok()?;
    let micros = fractional_micros(fraction);
    let micros = if value.starts_with('-') {
        -micros
    } else {
        micros
    };
    Some((
        seconds.checked_mul(1_000_000)?.checked_add(micros)?,
        text.get(end..)?,
    ))
}

pub(crate) fn calendar(text: &str) -> Option<(NaiveDateTime, i64, &str, &str)> {
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
        if digits == 0 {
            return None;
        }
        rest = fraction.get(digits..)?;
        fractional_micros(fraction.get(..digits)?)
    } else {
        0
    };
    let mut label = "";
    if let Some(tail) = rest.strip_prefix(' ') {
        let numeric = tail.starts_with(['+', '-']);
        let end = tail
            .char_indices()
            .find(|(at, c)| {
                let offset_colon = numeric
                    && *c == ':'
                    && matches!(*at, 3 | 6)
                    && tail
                        .as_bytes()
                        .get(at + 1..at + 3)
                        .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit))
                    && tail
                        .as_bytes()
                        .get(at + 3)
                        .is_none_or(|next| !next.is_ascii_digit());
                !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '/' | '_') || offset_colon)
            })
            .map_or(tail.len(), |(at, _)| at);
        if end != 0 {
            label = tail.get(..end)?;
            rest = tail.get(end..)?;
        }
    }
    Some((naive, micros, label, rest))
}

fn parse_naive(date: &str, clock: &str) -> Option<NaiveDateTime> {
    let year = date.get(..4)?.parse().ok()?;
    let month = date.get(5..7)?.parse().ok()?;
    let day = date.get(8..10)?.parse().ok()?;
    let hour = clock.get(..2)?.parse().ok()?;
    let minute = clock.get(3..5)?.parse().ok()?;
    let second = clock.get(6..8)?.parse().ok()?;
    if date.as_bytes().get(4) != Some(&b'-')
        || date.as_bytes().get(7) != Some(&b'-')
        || clock.as_bytes().get(2) != Some(&b':')
        || clock.as_bytes().get(5) != Some(&b':')
    {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)
}

fn fractional_micros(digits: &str) -> i64 {
    let mut micros = 0_i64;
    let mut scale = 100_000_i64;
    for byte in digits.bytes().take(6) {
        micros += i64::from(byte - b'0') * scale;
        scale /= 10;
    }
    micros
}

#[cfg(test)]
#[path = "tests/timestamp.rs"]
mod tests;
