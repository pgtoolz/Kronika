//! `log_destination = jsonlog`, available from PG 15.
//!
//! One record is one JSON object on one line: the writer escapes newlines, so
//! nothing here ever spans lines.

use serde_json::Value;

use super::{PgRecord, Severity};
use crate::text::bounded;
use crate::timestamp;

/// A JSON record is always one line.
pub(super) const fn continues(_open: &[String], _line: &str, _raw_quotes_odd: bool) -> bool {
    false
}

pub(super) fn parse(
    line: &str,
    zone: Option<&timestamp::LogTimezone>,
    now: i64,
) -> Option<PgRecord> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return None;
    };
    let severity = text(&value, "error_severity").and_then(Severity::parse)?;
    let message = text(&value, "message")?;
    let ts = text(&value, "timestamp")
        .and_then(|text| timestamp::parse(text, zone).ok())
        .filter(|(_, rest)| rest.is_empty())
        .map_or(now, |(ts, _)| ts);
    let mut parsed = PgRecord::new(ts, severity, message);
    parsed.sqlstate = text(&value, "state_code").and_then(bounded);
    parsed.detail = text(&value, "detail").and_then(bounded);
    parsed.hint = text(&value, "hint").and_then(bounded);
    parsed.context = text(&value, "context").and_then(bounded);
    parsed.statement = text(&value, "statement").and_then(bounded);
    parsed.database = text(&value, "dbname").and_then(bounded);
    parsed.username = text(&value, "user").and_then(bounded);
    Some(parsed)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
