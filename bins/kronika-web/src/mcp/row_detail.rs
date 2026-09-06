//! `kronika_get_row_detail`: one recorded row addressed by an opaque reference.

use std::sync::Arc;

use kronika_query::{QueryContext, execute_row_detail, validate_row_detail_ref};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::api::ApiError;
use crate::config::Config;
use crate::query_adapter::NativeDataset;

use super::catalog::RowDetailInput;
use super::semantics::{CancellationSink, detail_ref_error, mcp_error, mcp_structured};

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: RowDetailInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(_error) => {
            return mcp_error("invalid arguments: pass only one copied detail_ref string");
        }
    };
    let request = match validate_row_detail_ref(&input.detail_ref) {
        Ok(request) => request,
        Err(_error) => return mcp_error("invalid detail_ref"),
    };
    let dataset = match NativeDataset::from_root(&config.data_root) {
        Ok(dataset) => Arc::new(dataset),
        Err(error) => return detail_ref_error(&ApiError::from(error)),
    };
    let context = QueryContext::new(dataset, config.sources, config.synthetic_demo);
    let row = match execute_row_detail(&context, request, &CancellationSink::new(cancelled)) {
        Ok(row) => row,
        Err(error) => return detail_ref_error(&error),
    };
    mcp_structured(Value::Object(row.fields))
}
