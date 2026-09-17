//! Snapshot parameter parsing and cross-field validation.

use kronika_query::{Filter, Order, RelationGroup, SnapshotRequest, StatementScope};

use super::{
    DEFAULT_SNAPSHOT_PAGE_SIZE, MAX_FIELDS, MAX_FILTERS, MAX_ORDER_FIELDS,
    MAX_SEARCH_EXPRESSION_CHARS, MAX_SECTION_BYTES, MAX_SNAPSHOT_PAGE_SIZE, MAX_SNAPSHOT_SECTIONS,
    RouteError,
};

use crate::parameters::{
    bounded, decoded, number, pairs, relation_group, statement_scope, unsigned_32, unsigned_64,
};

#[expect(
    clippy::too_many_lines,
    reason = "the bounded snapshot query grammar is deliberately parsed in one strict pass"
)]
pub(super) fn parse_snapshot(segment_id: i64, query: &str) -> Result<SnapshotRequest, RouteError> {
    let mut at = None;
    let mut sections = Vec::new();
    let mut fields = Vec::new();
    let mut by = Vec::new();
    let mut direction = None;
    let mut group = None;
    let mut page_size = None;
    let mut cursor = None;
    let mut search = None;
    let mut first_match = None;
    let mut scope = None;
    let mut text = None;
    let mut filters: Vec<Filter> = Vec::new();
    let mut type_id = None;
    let mut row_ordinal = None;
    for (raw_name, raw_value) in pairs(query)? {
        match raw_name {
            "at" if at.is_none() => at = Some(number("at", raw_value)?),
            "section" => {
                let section = decoded("section", raw_value, false)?;
                if section.is_empty() || section.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                if sections.contains(&section) || sections.len() >= MAX_SNAPSHOT_SECTIONS {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                sections.push(section);
            }
            "field" => {
                let field = decoded("field", raw_value, true)?;
                if field.is_empty() || fields.contains(&field) || fields.len() >= MAX_FIELDS {
                    return Err(RouteError::BadParameter("field".to_owned()));
                }
                fields.push(field);
            }
            "by" => {
                let field = decoded("by", raw_value, true)?;
                if field.is_empty() || by.contains(&field) || by.len() >= MAX_ORDER_FIELDS {
                    return Err(RouteError::BadParameter("by".to_owned()));
                }
                by.push(field);
            }
            "direction" if direction.is_none() => {
                direction = Some(match raw_value {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter("direction".to_owned())),
                });
            }
            "group" if group.is_none() => {
                group = Some(relation_group(raw_value)?);
            }
            "type_id" if type_id.is_none() => {
                type_id = Some(unsigned_32("type_id", raw_value)?);
            }
            "row_ordinal" if row_ordinal.is_none() => {
                row_ordinal = Some(unsigned_64("row_ordinal", raw_value)?);
            }
            "text" if text.is_none() => {
                let kept = number("text", raw_value)?;
                if kept <= 0 {
                    return Err(RouteError::BadParameter("text".to_owned()));
                }
                text = Some(kept.unsigned_abs());
            }
            "page_size" if page_size.is_none() => {
                page_size = Some(bounded("page_size", raw_value, MAX_SNAPSHOT_PAGE_SIZE)?);
            }
            "cursor" if cursor.is_none() => {
                let value = decoded("cursor", raw_value, true)?;
                if value.is_empty() {
                    return Err(RouteError::BadParameter("cursor".to_owned()));
                }
                cursor = Some(value);
            }
            "search" if search.is_none() => search = Some(snapshot_search(raw_value)?),
            "scope" if scope.is_none() => scope = Some(statement_scope(raw_value)?),
            "first_match" if first_match.is_none() => {
                if raw_value != "1" {
                    return Err(RouteError::BadParameter("first_match".to_owned()));
                }
                first_match = Some(true);
            }
            other => {
                let name = decoded("parameter", other, true)?;
                let Some(column) = name.strip_prefix("where.") else {
                    return Err(RouteError::BadParameter(name));
                };
                if column.is_empty()
                    || filters.len() >= MAX_FILTERS
                    || filters.iter().any(|filter| filter.column == column)
                {
                    return Err(RouteError::BadParameter("where".to_owned()));
                }
                filters.push(Filter {
                    column: column.to_owned(),
                    value: decoded(&name, raw_value, true)?,
                });
            }
        }
    }
    let first_match = first_match.unwrap_or(false);
    if first_match
        && (sections.as_slice() != ["pg_stat_statements"]
            || fields.as_slice() != ["query"]
            || page_size != Some(1)
            || cursor.is_some()
            || search.is_none()
            || text.is_some()
            || !filters.is_empty()
            || type_id.is_some()
            || row_ordinal.is_some()
            || !by.is_empty()
            || direction.is_some()
            || group.is_some())
    {
        return Err(RouteError::BadParameter("first_match".to_owned()));
    }
    let paged = page_size.is_some()
        || cursor.is_some()
        || search.is_some()
        || !by.is_empty()
        || direction.is_some()
        || group.is_some();
    let scope = scope.unwrap_or_default();
    if scope == StatementScope::Workload
        && (sections.len() != 1
            || !scope.allows_rows(&sections[0])
            || !paged
            || first_match
            || row_ordinal.is_some()
            || group.is_some())
    {
        return Err(RouteError::BadParameter("scope".to_owned()));
    }
    validate_snapshot_shape(&sections, paged, &filters, type_id, row_ordinal, group)?;
    Ok(SnapshotRequest {
        segment_id,
        at: at.ok_or_else(|| RouteError::BadParameter("at".to_owned()))?,
        sections,
        fields,
        by,
        direction: direction.unwrap_or(Order::Desc),
        group,
        page_size: paged.then_some(page_size.unwrap_or(DEFAULT_SNAPSHOT_PAGE_SIZE)),
        cursor,
        search,
        first_match,
        text,
        filters,
        type_id,
        row_ordinal,
        scope,
    })
}

fn snapshot_search(raw: &str) -> Result<String, RouteError> {
    let value = decoded("search", raw, true)?;
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_SEARCH_EXPRESSION_CHARS {
        return Err(RouteError::BadParameter("search".to_owned()));
    }
    Ok(value.to_owned())
}

fn validate_snapshot_shape(
    sections: &[String],
    paged: bool,
    filters: &[Filter],
    type_id: Option<u32>,
    row_ordinal: Option<u64>,
    group: Option<RelationGroup>,
) -> Result<(), RouteError> {
    if sections.is_empty() {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if sections.len() != 1
        && (paged || !filters.is_empty() || type_id.is_some() || row_ordinal.is_some())
    {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    if row_ordinal.is_some() && (type_id.is_none() || paged || !filters.is_empty()) {
        return Err(RouteError::BadParameter("row_ordinal".to_owned()));
    }
    if group.is_some() && type_id.is_some() {
        return Err(RouteError::BadParameter("type_id".to_owned()));
    }
    if group.is_some()
        && (!paged
            || row_ordinal.is_some()
            || !matches!(sections, [section] if section == "pg_stat_user_tables" || section == "pg_stat_user_indexes"))
    {
        return Err(RouteError::BadParameter("group".to_owned()));
    }
    Ok(())
}
