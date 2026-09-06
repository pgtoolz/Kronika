//! Strict public MCP timestamp expressions.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use kronika_query::snapshot::SnapshotPoint;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use kronika_query::TimeRange;

const VALID_FORMS: &str = "a JSON integer or canonical signed decimal-string i64 Unix timestamp in microseconds, RFC 3339 with Z or a numeric UTC offset, now, or now-N{us,ms,s,m,h,d,w}";

/// Shared MCP time input. Decimal-string `recorded_from` and `recorded_to`
/// outputs can be passed unchanged to `from`, `to`, and `at`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum TimeSpecInput {
    /// Exact Unix timestamp in microseconds as a JSON integer.
    UnixMicros(i64),
    /// Canonical signed decimal-string i64 Unix microseconds, RFC 3339, `now`,
    /// or a fixed-duration expression such as `now-1h`.
    Expression(String),
}

impl TimeSpecInput {
    pub(crate) fn resolve(&self, now: i64) -> Result<i64, TimeSpecError> {
        match self {
            Self::UnixMicros(timestamp) => Ok(*timestamp),
            Self::Expression(expression) => resolve_expression(expression, now),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TimeSpecError(String);

impl std::fmt::Display for TimeSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; expected {VALID_FORMS}", self.0)
    }
}

impl std::error::Error for TimeSpecError {}

pub(crate) fn resolve_range(
    from: &TimeSpecInput,
    to: &TimeSpecInput,
) -> Result<TimeRange, TimeSpecError> {
    resolve_range_with(from, to, system_now_micros)
}

pub(crate) fn resolve_bounded_range(
    from: &TimeSpecInput,
    to: &TimeSpecInput,
    max_width: i64,
) -> Result<TimeRange, TimeSpecError> {
    let range = resolve_range_with(from, to, system_now_micros)?;
    TimeRange::bounded(range.from(), range.to_exclusive(), max_width).map_err(TimeSpecError)
}

pub(crate) fn resolve_point(at: Option<&TimeSpecInput>) -> Result<SnapshotPoint, TimeSpecError> {
    let Some(at) = at else {
        return Ok(SnapshotPoint::LatestRecorded);
    };
    let now = system_now_micros()?;
    at.resolve(now).map(SnapshotPoint::At)
}

fn resolve_range_with(
    from: &TimeSpecInput,
    to: &TimeSpecInput,
    clock: impl FnOnce() -> Result<i64, TimeSpecError>,
) -> Result<TimeRange, TimeSpecError> {
    let now = clock()?;
    let from = from.resolve(now)?;
    let to = to.resolve(now)?;
    TimeRange::new(from, to).map_err(TimeSpecError)
}

fn resolve_expression(expression: &str, now: i64) -> Result<i64, TimeSpecError> {
    if let Ok(timestamp) = expression.parse::<i64>()
        && timestamp.to_string() == expression
    {
        return Ok(timestamp);
    }
    if expression == "now" {
        return Ok(now);
    }
    if let Some(relative) = expression.strip_prefix("now-") {
        return resolve_relative(relative, now);
    }
    resolve_rfc3339(expression)
}

fn resolve_relative(relative: &str, now: i64) -> Result<i64, TimeSpecError> {
    let (digits, multiplier) = [
        ("us", 1_i64),
        ("ms", 1_000),
        ("s", 1_000_000),
        ("m", 60_000_000),
        ("h", 3_600_000_000),
        ("d", 86_400_000_000),
        ("w", 604_800_000_000),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| {
        relative
            .strip_suffix(suffix)
            .map(|digits| (digits, multiplier))
    })
    .ok_or_else(|| TimeSpecError(format!("invalid timestamp expression {relative:?}")))?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TimeSpecError(format!(
            "invalid relative timestamp {relative:?}"
        )));
    }
    let count = digits
        .parse::<i64>()
        .map_err(|_error| TimeSpecError(format!("relative timestamp {relative:?} overflows")))?;
    let duration = count
        .checked_mul(multiplier)
        .ok_or_else(|| TimeSpecError(format!("relative timestamp {relative:?} overflows")))?;
    now.checked_sub(duration)
        .ok_or_else(|| TimeSpecError(format!("relative timestamp {relative:?} overflows")))
}

fn resolve_rfc3339(expression: &str) -> Result<i64, TimeSpecError> {
    let numeric_offset = expression
        .as_bytes()
        .get(expression.len().saturating_sub(6)..)
        .is_some_and(|suffix| {
            suffix.len() == 6
                && matches!(suffix[0], b'+' | b'-')
                && suffix[1].is_ascii_digit()
                && suffix[2].is_ascii_digit()
                && suffix[3] == b':'
                && suffix[4].is_ascii_digit()
                && suffix[5].is_ascii_digit()
        });
    if expression.bytes().any(|byte| byte.is_ascii_whitespace())
        || !(expression.contains('T') || expression.contains('t'))
        || !(expression.ends_with('Z') || numeric_offset)
    {
        return Err(TimeSpecError(format!(
            "invalid timestamp expression {expression:?}"
        )));
    }
    let offset_start = expression.len() - if expression.ends_with('Z') { 1 } else { 6 };
    let timestamp = &expression.as_bytes()[..offset_start];
    if let Some(dot) = timestamp.iter().rposition(|byte| *byte == b'.')
        && timestamp[dot + 1..]
            .get(6..)
            .is_some_and(|discarded| discarded.iter().any(|digit| *digit != b'0'))
    {
        return Err(TimeSpecError(format!(
            "timestamp {expression:?} is not exactly representable in microseconds"
        )));
    }
    let parsed = DateTime::parse_from_rfc3339(expression)
        .map_err(|_error| TimeSpecError(format!("invalid timestamp expression {expression:?}")))?;
    if parsed.timestamp_subsec_nanos() % 1_000 != 0 {
        return Err(TimeSpecError(format!(
            "timestamp {expression:?} is not exactly representable in microseconds"
        )));
    }
    Ok(parsed.timestamp_micros())
}

fn system_now_micros() -> Result<i64, TimeSpecError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_error| TimeSpecError("system clock precedes the Unix epoch".to_owned()))?;
    let micros = elapsed.as_micros();
    i64::try_from(micros).map_err(|_error| TimeSpecError("system clock overflows i64".to_owned()))
}

#[cfg(test)]
mod tests;
