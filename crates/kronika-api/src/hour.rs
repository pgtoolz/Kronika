//! Hour selection, window bounds, and relation-series parameters.

use kronika_query::{
    CatalogRequest, Filter, HourPart, HourRequest, HourSeriesRequest, RelationGroup,
    StatementScope, Window,
};

use super::{MAX_SECTION_BYTES, RouteError};

use crate::parameters::{
    active_cursor, data_parameters, decoded, number, pairs, relation_group, statement_scope,
    unsigned_32,
};

/// Only the `postgresql_summary` series folds statements, so only it accepts a scope.
fn summary_scope(
    scope: Option<StatementScope>,
    section: Option<&str>,
) -> Result<StatementScope, RouteError> {
    let scope = scope.unwrap_or_default();
    if !scope.allows_series(section) {
        return Err(RouteError::BadParameter("scope".to_owned()));
    }
    Ok(scope)
}
pub(super) fn parse_catalog(query: &str) -> Result<CatalogRequest, RouteError> {
    let mut window = Window::default();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    valid_window(window).map(|window| CatalogRequest { window })
}

pub(super) fn parse_hour(query: &str) -> Result<HourRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut window = Window::default();
    let mut section = None;
    let mut type_id = None;
    let mut group = None;
    let mut part = HourPart::Combined;
    let mut segments = None;
    let mut active = None;
    let mut saw_part = false;
    let mut scope = None;
    for (name, value) in extras {
        match name.as_str() {
            "scope" if scope.is_none() => scope = Some(statement_scope(&value)?),
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            "group" if group.is_none() => {
                group = Some(relation_group(&value)?);
            }
            "part" if !saw_part => {
                part = match value.as_str() {
                    "base" => HourPart::Base,
                    "lanes" => HourPart::Lanes,
                    _ => return Err(RouteError::BadParameter("part".to_owned())),
                };
                saw_part = true;
            }
            "segments" if segments.is_none() => segments = Some(segment_ids(&value)?),
            "active" if active.is_none() => active = Some(active_cursor("active", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let window = valid_window(window)?;
    let scope = summary_scope(scope, section.as_deref())?;
    let Some(section) = section else {
        if fields.is_empty() && filters.is_empty() && type_id.is_none() && group.is_none() {
            if part == HourPart::Lanes && segments.is_none() {
                return Err(RouteError::BadParameter("segments".to_owned()));
            }
            if part == HourPart::Lanes && (window.from.is_none() || window.to.is_none()) {
                return Err(RouteError::BadParameter(
                    if window.from.is_none() { "from" } else { "to" }.to_owned(),
                ));
            }
            if part != HourPart::Lanes && (segments.is_some() || active.is_some()) {
                return Err(RouteError::BadParameter(
                    if segments.is_some() {
                        "segments"
                    } else {
                        "active"
                    }
                    .to_owned(),
                ));
            }
            return Ok(HourRequest {
                window,
                series: None,
                part,
                segments,
                active,
            });
        }
        return Err(RouteError::BadParameter("section".to_owned()));
    };
    if part != HourPart::Combined || segments.is_some() || active.is_some() {
        return Err(RouteError::BadParameter(if segments.is_some() {
            "segments".to_owned()
        } else if active.is_some() {
            "active".to_owned()
        } else {
            "part".to_owned()
        }));
    }
    validate_relation_series(&section, &fields, &filters, type_id, group)?;
    Ok(HourRequest {
        window,
        series: Some(HourSeriesRequest {
            section,
            fields,
            filters,
            type_id,
            group,
            scope,
        }),
        part,
        segments,
        active,
    })
}

fn valid_window(window: Window) -> Result<Window, RouteError> {
    if window
        .from
        .zip(window.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    Ok(window)
}

fn segment_ids(value: &str) -> Result<Vec<i64>, RouteError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for value in value.split(',') {
        let id = number("segments", value)?;
        if ids.contains(&id) {
            return Err(RouteError::BadParameter("segments".to_owned()));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn validate_relation_series(
    section: &str,
    fields: &[String],
    filters: &[Filter],
    type_id: Option<u32>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    let Some(group) = group else {
        return Ok(());
    };
    if type_id.is_some()
        || fields.is_empty()
        || !matches!(section, "pg_stat_user_tables" | "pg_stat_user_indexes")
        || group == RelationGroup::Object
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    let required: &[&str] = match group {
        RelationGroup::Database => &["datid"],
        RelationGroup::Schema => &["datid", "schemaname"],
        RelationGroup::Tablespace => &["tablespace_oid"],
        RelationGroup::Object => unreachable!(),
    };
    if filters.len() != required.len()
        || required
            .iter()
            .any(|name| !filters.iter().any(|filter| filter.column == *name))
    {
        return Err(RouteError::BadParameter("where".to_owned()));
    }
    if group == RelationGroup::Tablespace
        && filters[0]
            .value
            .parse::<u32>()
            .ok()
            .is_none_or(|oid| oid == 0)
    {
        return Err(RouteError::BadParameter("where.tablespace_oid".to_owned()));
    }
    Ok(())
}
