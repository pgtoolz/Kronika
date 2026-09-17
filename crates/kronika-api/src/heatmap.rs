//! Heatmap parameter parsing and output bounds.

use kronika_query::{
    DEFAULT_TOP as DEFAULT_HEATMAP_TOP, MAX_FIELDS as MAX_HEATMAP_FIELDS,
    MAX_TOP as MAX_HEATMAP_TOP,
};

use super::{
    DEFAULT_HEATMAP_COLUMNS, HeatmapRequest, MAX_HEATMAP_COLUMNS, MAX_HEATMAP_GROUP,
    MAX_SECTION_BYTES, RouteError,
};

use crate::parameters::{bounded, decoded, number, pairs, statement_scope, unsigned_32};

pub(super) fn parse_heatmap(query: &str) -> Result<HeatmapRequest, RouteError> {
    let mut from = None;
    let mut to = None;
    let mut section = None;
    let mut fields: Vec<String> = Vec::new();
    let mut columns = None;
    let mut top = None;
    let mut group: Vec<String> = Vec::new();
    let mut type_id = None;
    let mut scope = None;
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if from.is_none() => from = Some(number("from", &value)?),
            "to" if to.is_none() => to = Some(number("to", &value)?),
            "section" if section.is_none() => {
                if value.is_empty() || value.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                section = Some(value);
            }
            "scope" if scope.is_none() => scope = Some(statement_scope(&value)?),
            "field" => {
                if value.is_empty() || fields.contains(&value) || fields.len() >= MAX_HEATMAP_FIELDS
                {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(value);
            }
            "columns" if columns.is_none() => {
                columns = Some(bounded("columns", &value, MAX_HEATMAP_COLUMNS)?);
            }
            "top" if top.is_none() => top = Some(bounded("top", &value, MAX_HEATMAP_TOP)?),
            "group" => {
                if value.is_empty() || group.contains(&value) || group.len() >= MAX_HEATMAP_GROUP {
                    return Err(RouteError::BadParameter("group".to_owned()));
                }
                group.push(value);
            }
            "type_id" if type_id.is_none() => type_id = Some(unsigned_32("type_id", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    let from = from.ok_or_else(|| RouteError::BadParameter("from".to_owned()))?;
    let to = to.ok_or_else(|| RouteError::BadParameter("to".to_owned()))?;
    if from > to {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    if fields.is_empty() {
        return Err(RouteError::BadParameter("field".to_owned()));
    }
    Ok(HeatmapRequest {
        from,
        to,
        section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
        fields,
        columns: columns.unwrap_or(DEFAULT_HEATMAP_COLUMNS),
        top: top.unwrap_or(DEFAULT_HEATMAP_TOP),
        group,
        type_id,
        scope: scope.unwrap_or_default(),
    })
}
