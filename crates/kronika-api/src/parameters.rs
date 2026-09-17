//! Shared strict query decoding and scalar validation.

use kronika_query::{ActiveCursor, Filter, RelationGroup, StatementScope};

use super::{MAX_FIELDS, MAX_FILTERS, RouteError};

pub(super) fn pairs(query: &str) -> Result<Vec<(&str, &str)>, RouteError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|part| {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            Ok(part.split_once('=').unwrap_or((part, "")))
        })
        .collect()
}

pub(super) fn decoded(name: &str, value: &str, plus_as_space: bool) -> Result<String, RouteError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0_usize;
    while at < bytes.len() {
        match bytes[at] {
            b'+' if plus_as_space => {
                out.push(b' ');
                at += 1;
            }
            b'%' => {
                let raw = bytes
                    .get(at + 1..at + 3)
                    .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
                let text = std::str::from_utf8(raw)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                let byte = u8::from_str_radix(text, 16)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                out.push(byte);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

pub(super) fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

pub(super) fn unsigned_32(name: &str, value: &str) -> Result<u32, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

pub(super) fn unsigned_64(name: &str, value: &str) -> Result<u64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}
pub(super) fn statement_scope(value: &str) -> Result<StatementScope, RouteError> {
    StatementScope::parse(value).ok_or_else(|| RouteError::BadParameter("scope".to_owned()))
}

pub(super) fn relation_group(value: &str) -> Result<RelationGroup, RouteError> {
    RelationGroup::parse(value).ok_or_else(|| RouteError::BadParameter("group".to_owned()))
}
pub(super) fn bounded(name: &str, value: &str, cap: usize) -> Result<usize, RouteError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|parsed| (1..=cap).contains(parsed))
        .ok_or_else(|| RouteError::BadParameter(name.to_owned()))
}

pub(super) fn active_cursor(name: &str, value: &str) -> Result<ActiveCursor, RouteError> {
    let (segment_id, wal_position) = value
        .split_once(',')
        .filter(|(segment_id, wal_position)| {
            !segment_id.is_empty() && !wal_position.is_empty() && !wal_position.contains(',')
        })
        .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
    Ok(ActiveCursor {
        segment_id: number(name, segment_id)?,
        wal_position: wal_position
            .parse()
            .map_err(|_error| RouteError::BadParameter(name.to_owned()))?,
    })
}

type Extra = (String, String);
type DataParameters = (Vec<String>, Vec<Filter>, Vec<Extra>);

pub(super) fn data_parameters(query: &str) -> Result<DataParameters, RouteError> {
    let mut fields = Vec::new();
    let mut filters = Vec::new();
    let mut extras = Vec::new();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        if name == "field" {
            if value.is_empty() || fields.len() >= MAX_FIELDS || fields.contains(&value) {
                return Err(RouteError::BadParameter("field".to_owned()));
            }
            fields.push(value);
        } else if let Some(column) = name.strip_prefix("where.") {
            if column.is_empty()
                || filters.len() >= MAX_FILTERS
                || filters
                    .iter()
                    .any(|filter: &Filter| filter.column == column)
            {
                return Err(RouteError::BadParameter("where".to_owned()));
            }
            filters.push(Filter {
                column: column.to_owned(),
                value,
            });
        } else {
            extras.push((name, value));
        }
    }
    Ok((fields, filters, extras))
}
