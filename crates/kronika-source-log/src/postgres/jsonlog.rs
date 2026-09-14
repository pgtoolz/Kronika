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
) -> Result<Option<PgRecord>, &'static str> {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Ok(None);
    };
    let Some(severity) = text(&value, "error_severity").and_then(Severity::parse) else {
        return Ok(None);
    };
    let Some(message) = text(&value, "message") else {
        return Ok(None);
    };
    let (ts, rest) = timestamp::parse(text(&value, "timestamp").ok_or(timestamp::INVALID)?, zone)?;
    if !rest.is_empty() {
        return Err(timestamp::INVALID);
    }
    let mut parsed = PgRecord::new(ts, severity, message);
    parsed.sqlstate = text(&value, "state_code").and_then(bounded);
    parsed.detail = text(&value, "detail").and_then(bounded);
    parsed.hint = text(&value, "hint").and_then(bounded);
    parsed.context = text(&value, "context").and_then(bounded);
    parsed.statement = text(&value, "statement").and_then(bounded);
    parsed.database = text(&value, "dbname").and_then(bounded);
    parsed.username = text(&value, "user").and_then(bounded);
    Ok(Some(parsed))
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
