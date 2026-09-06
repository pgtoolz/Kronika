//! `kronika_list_recorded_sections`: logical sections in recorded data.

use std::sync::Arc;

use kronika_query::{
    CatalogField, CatalogRequest, CatalogSection, QueryContext, Window, catalog_facts,
};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::config::Config;

use super::semantics::{mcp_error, mcp_structured, storage_error};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    _cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: super::catalog::GetContextInput =
        match serde_json::from_value(Value::Object(arguments)) {
            Ok(input) => input,
            Err(error) => {
                return super::semantics::invalid_arguments(
                    super::catalog::GET_CONTEXT_TOOL,
                    "an optional section name narrows the answer to one section",
                    error,
                );
            }
        };
    let dataset = match crate::query_adapter::NativeDataset::from_root(&config.data_root) {
        Ok(dataset) => Arc::new(dataset),
        Err(error) => return storage_error(&crate::api::ApiError::from(error)),
    };
    let context = QueryContext::new(dataset, config.sources, config.synthetic_demo);
    let facts = match catalog_facts(
        &context,
        CatalogRequest {
            window: Window::default(),
        },
    ) {
        Ok(facts) => facts,
        Err(error) => return storage_error(&error),
    };
    let range = match exclusive_recorded_range(facts.recorded_range) {
        Ok(range) => range,
        Err(error) => return mcp_error(error),
    };
    if let Some(wanted) = &input.section
        && !facts
            .sections
            .iter()
            .any(|section| section.logical_name == wanted.as_str())
    {
        let recorded = facts
            .sections
            .iter()
            .map(|section| section.logical_name.to_owned())
            .collect::<Vec<_>>();
        return super::semantics::mcp_error_with(
            format!(
                "no recorded section named {wanted:?}; recorded: {}",
                recorded.join(", ")
            ),
            recorded,
        );
    }
    let sections = facts
        .sections
        .iter()
        .filter(|section| {
            input
                .section
                .as_deref()
                .is_none_or(|wanted| section.logical_name == wanted)
        })
        .map(section_value)
        .collect::<Vec<_>>();
    mcp_structured(json!({
        "recorded_from": range.map(|(from, _to)| from.to_string()),
        "recorded_to": range.map(|(_from, to)| to.to_string()),
        "sections": sections,
    }))
}

fn section_value(section: &CatalogSection) -> Value {
    json!({
        "logical_name": section.logical_name,
        "source_family": section.source_family,
        "rows": section.rows.to_string(),
        "bytes": section.bytes.to_string(),
        "fields": section.fields.iter().map(field_value).collect::<Vec<_>>(),
    })
}

fn field_value(field: &CatalogField) -> Value {
    json!({
        "name": field.name,
        "class": field.class,
        "unit": field.unit,
    })
}

fn exclusive_recorded_range(range: Option<(i64, i64)>) -> Result<Option<(i64, i64)>, &'static str> {
    range
        .map(|(from, to)| {
            to.checked_add(1)
                .map(|to_exclusive| (from, to_exclusive))
                .ok_or("last recorded timestamp cannot form an exclusive upper bound")
        })
        .transpose()
}

#[cfg(test)]
mod tests;
