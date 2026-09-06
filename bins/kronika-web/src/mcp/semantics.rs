//! `CallToolResult` helpers and decimal-string serialization for 64-bit integers.

use std::collections::BTreeMap;

use rmcp::model::{CallToolResult, ContentBlock};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::budget::ByteBudget;
use crate::route::MAX_QUERY_BYTES;

pub(crate) struct CancellationSink<'a> {
    cancelled: &'a dyn Fn() -> bool,
}

impl std::fmt::Debug for CancellationSink<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancellationSink").finish_non_exhaustive()
    }
}

impl<'a> CancellationSink<'a> {
    pub(crate) const fn new(cancelled: &'a dyn Fn() -> bool) -> Self {
        Self { cancelled }
    }
}

impl kronika_query::QuerySink for CancellationSink<'_> {
    fn record(&mut self, _bytes: Vec<u8>) -> bool {
        false
    }

    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct FinderOutput {
    #[schemars(with = "Vec<BTreeMap<String, Value>>")]
    rows: Vec<Value>,
    truncated: bool,
}

pub(crate) fn finder_output(rows: Vec<Value>, truncated: bool) -> Value {
    serde_json::json!(FinderOutput { rows, truncated })
}

/// Replaces an internal HTTP row locator with its public MCP reference.
pub(crate) fn set_detail_ref(value: &mut Value, detail_ref: String) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "detail-bearing result is not an object".to_owned())?;
    if object.remove("detail_locator").is_none() || object.contains_key("detail_ref") {
        return Err("detail-bearing result has an invalid internal shape".to_owned());
    }
    object.insert("detail_ref".to_owned(), Value::String(detail_ref));
    Ok(())
}

/// Returns an in-band tool execution error. The text is mirrored into
/// `structuredContent` as `{"record":"error","message":…}`, so a client
/// reading only the structure sees the error instead of an empty result.
pub(crate) fn mcp_error(message: impl Into<String>) -> CallToolResult {
    mcp_error_with(message, Vec::new())
}

/// An error whose structured mirror also carries `valid_options` — the
/// choices the refused input can be replaced with, ready to pick without
/// parsing the text.
pub(crate) fn mcp_error_with(
    message: impl Into<String>,
    valid_options: Vec<String>,
) -> CallToolResult {
    let message = message.into();
    let mut body = Map::new();
    body.insert("record".to_owned(), Value::String("error".to_owned()));
    body.insert("message".to_owned(), Value::String(message.clone()));
    if !valid_options.is_empty() {
        body.insert(
            "valid_options".to_owned(),
            Value::Array(valid_options.into_iter().map(Value::String).collect()),
        );
    }
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.structured_content = Some(Value::Object(body));
    result
}

/// A batch refusal naming the zero-based ranking whose validation or resource
/// check failed.
pub(crate) fn mcp_error_indexed(
    message: impl Into<String>,
    ranking_index: usize,
) -> CallToolResult {
    mcp_error_indexed_with(message, ranking_index, Vec::new())
}

/// A batch refusal with both its zero-based ranking and known replacements.
pub(crate) fn mcp_error_indexed_with(
    message: impl Into<String>,
    ranking_index: usize,
    valid_options: Vec<String>,
) -> CallToolResult {
    let message = message.into();
    let mut result = mcp_error_with(message, valid_options);
    if let Some(Value::Object(body)) = result.structured_content.as_mut() {
        body.insert("ranking_index".to_owned(), serde_json::json!(ranking_index));
    }
    result
}

/// A storage error as a tool answer; an unknown section or column also
/// names the tool that lists the valid ones.
pub(crate) fn storage_error(error: &crate::api::ApiError) -> CallToolResult {
    let hinted = matches!(
        error,
        crate::api::ApiError::NoSuchSection | crate::api::ApiError::NoSuchColumn(_)
    );
    let mut message = storage_error_message(error);
    if hinted {
        message
            .push_str("; kronika_list_recorded_sections lists recorded sections and their fields");
    }
    mcp_error(message)
}

/// Maps typed storage failures to fail-closed public MCP text.
pub(crate) fn storage_error_message(error: &crate::api::ApiError) -> String {
    match error {
        crate::api::ApiError::NoSuchSegment => {
            "no stored data is available at the requested time".to_owned()
        }
        crate::api::ApiError::NoSuchSection
        | crate::api::ApiError::MixedUnits(_)
        | crate::api::ApiError::Cancelled => error.to_string(),
        crate::api::ApiError::NoSuchColumn(_) => "no such stored-data field".to_owned(),
        crate::api::ApiError::BadFilter(_) | crate::api::ApiError::BadCursor => {
            "invalid stored-data request".to_owned()
        }
        crate::api::ApiError::BadLocator(_) => "could not produce detail_ref".to_owned(),
        _ => "could not read stored data".to_owned(),
    }
}

/// Distinguishes a failed lookup from cancellation or an unreadable store.
pub(crate) fn detail_ref_error(error: &crate::api::ApiError) -> CallToolResult {
    match error {
        crate::api::ApiError::Cancelled | crate::api::ApiError::Unreadable(_) => {
            storage_error(error)
        }
        _ => mcp_error("detail_ref does not identify one recorded row"),
    }
}

pub(crate) fn finder_storage_error(
    logical_name: &str,
    error: &crate::api::ApiError,
) -> CallToolResult {
    if matches!(error, crate::api::ApiError::NoSuchColumn(_)) {
        return mcp_error(format!(
            "no such sort field for {logical_name}; kronika_list_recorded_sections lists the section's fields"
        ));
    }
    storage_error(error)
}

/// Rejected tool arguments with the tool's one-line usage appended: the
/// serde text alone names a field, not the shape of a working call.
pub(crate) fn invalid_arguments(
    tool: &str,
    usage: &str,
    error: impl std::fmt::Display,
) -> CallToolResult {
    mcp_error(format!(
        "invalid arguments for {tool}: {error}. Usage: {usage}"
    ))
}

/// Accepts the inclusive range `1..=cap` and never clamps.
pub(crate) fn bounded_limit(name: &str, value: u32, cap: usize) -> Result<usize, CallToolResult> {
    let value = value as usize;
    if (1..=cap).contains(&value) {
        Ok(value)
    } else {
        Err(mcp_error(format!(
            "{name} must be between 1 and {cap}, got {value}"
        )))
    }
}

pub(crate) fn arguments_within_budget(arguments: &Map<String, Value>) -> bool {
    let mut budget = ByteBudget::new(MAX_QUERY_BYTES);
    serde_json::to_writer(&mut budget, arguments).is_ok()
}

/// Places the same complete JSON value in `content` and `structuredContent`.
pub(crate) fn mcp_structured(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

#[cfg(test)]
mod tests;
