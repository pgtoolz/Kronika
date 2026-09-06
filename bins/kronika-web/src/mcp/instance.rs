//! `kronika_get_instance`: the newest recorded host facts and `PostgreSQL`
//! server settings.

use std::collections::BTreeMap;

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use kronika_query::snapshot::PlainRowOut;

use super::catalog::{GetInstanceInput, SettingsScopeInput};
use super::postgresql::{plain_row_to_json, plain_rows};
use super::semantics::{mcp_error, mcp_structured};
use crate::config::Config;

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct InstanceOutput {
    #[schemars(with = "Option<BTreeMap<String, Value>>")]
    host: Option<Value>,
    host_as_of: Option<String>,
    #[schemars(with = "Vec<BTreeMap<String, Value>>")]
    postgresql_settings: Vec<Value>,
    settings_as_of: Option<String>,
    settings_scope: SettingsScopeInput,
    settings_returned_count: String,
    settings_defaults_omitted: bool,
    settings_request_all: AllSettingsRequest,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct AllSettingsRequest {
    settings: AllSettingsScope,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum AllSettingsScope {
    All,
}

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: GetInstanceInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::GET_INSTANCE_TOOL,
                "settings is optional: non_default (the default) or all",
                error,
            );
        }
    };
    let host = match newest_rows(config, "instance_metadata", cancelled) {
        Ok(part) => part,
        Err(error) => return error,
    };
    let settings = match newest_rows(config, "pg_settings", cancelled) {
        Ok(part) => part,
        Err(error) => return error,
    };

    // The newest recorded host row: layouts can each contribute their latest
    // observation, so pick the most recent by the row's own `at`.
    let host_row = match host
        .rows
        .into_iter()
        .max_by_key(|row| row.at)
        .map(|row| plain_row_to_json("instance_metadata", row))
        .transpose()
    {
        Ok(row) => row,
        Err(_error) => return mcp_error("could not produce detail_ref"),
    };
    let (selected, defaults_omitted) = select_settings(settings.rows, input.settings);
    let selected = match selected
        .into_iter()
        .map(|row| plain_row_to_json("pg_settings", row))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(rows) => rows,
        Err(_error) => return mcp_error("could not produce detail_ref"),
    };
    let row_count = selected.len();
    let output = InstanceOutput {
        host: host_row,
        host_as_of: host.as_of.map(|at| at.to_string()),
        postgresql_settings: selected,
        settings_as_of: settings.as_of.map(|at| at.to_string()),
        settings_scope: input.settings,
        settings_returned_count: row_count.to_string(),
        settings_defaults_omitted: defaults_omitted,
        settings_request_all: AllSettingsRequest {
            settings: AllSettingsScope::All,
        },
    };
    match serde_json::to_value(output) {
        Ok(output) => mcp_structured(output),
        Err(_error) => mcp_error("could not encode instance result"),
    }
}

fn select_settings(rows: Vec<PlainRowOut>, scope: SettingsScopeInput) -> (Vec<PlainRowOut>, bool) {
    let mut defaults_omitted = false;
    let rows = rows
        .into_iter()
        .filter(|row| {
            let is_default = matches!(
                row.fields.get("source"),
                Some(Value::String(source)) if source == "default"
            );
            let keep = scope == SettingsScopeInput::All || !is_default;
            defaults_omitted |= !keep;
            keep
        })
        .collect();
    (rows, defaults_omitted)
}

struct Part {
    rows: Vec<PlainRowOut>,
    as_of: Option<i64>,
}

/// One section's newest recorded rows through the shared plain pipeline; a
/// section recorded by no segment yields an empty part, not an error.
fn newest_rows(
    config: &Config,
    logical_name: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Part, CallToolResult> {
    let Some(result) = plain_rows(logical_name, config, cancelled)? else {
        return Ok(Part {
            rows: Vec::new(),
            as_of: None,
        });
    };
    Ok(Part {
        rows: result.rows,
        as_of: result.as_of,
    })
}

#[cfg(test)]
mod tests;
