//! `kronika_find_processes`: bounded sorting and filtering of recorded rows.

use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use kronika_query::snapshot::{
    FinderOrder, FinderQuery, FinderSurface, ProcessRowOut, SnapshotPoint, execute_processes,
};

use crate::config::Config;
use crate::route::MAX_SNAPSHOT_PAGE_SIZE;

use super::catalog::{ProcessesInput, SortInput};
use super::filter::{FilterInput, build_search};
use super::semantics::{bounded_limit, finder_output, mcp_structured};
use super::time::resolve_point;

const LOGICAL_NAME: &str = "os_process";

pub(crate) fn call(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: ProcessesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::FIND_PROCESSES_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let point = match resolve_point(input.at.as_ref()) {
        Ok(point) => point,
        Err(error) => {
            return super::semantics::invalid_arguments(
                super::catalog::FIND_PROCESSES_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    call_with(
        config,
        point,
        &input.filters,
        input.sort,
        input.limit,
        cancelled,
    )
}

fn call_with(
    config: &Config,
    point: SnapshotPoint,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let limit = match bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE) {
        Ok(limit) => limit,
        Err(error) => return error,
    };
    let search = match build_search(LOGICAL_NAME, filters) {
        Ok(search) => search,
        Err(refusal) => return refusal.into_error(),
    };
    let query = FinderQuery {
        surface: FinderSurface::Processes,
        point,
        search,
        order: sort.map(|sort| FinderOrder {
            field: sort.field,
            direction: sort.direction.into(),
        }),
        group: None,
        limit,
    };
    super::run_finder_query(
        config,
        LOGICAL_NAME,
        |context| execute_processes(context, &query, cancelled),
        |result| {
            let rows: Vec<Value> = match result.rows.into_iter().map(row_to_json).collect() {
                Ok(rows) => rows,
                Err(_error) => {
                    return super::semantics::mcp_error("could not produce detail_ref");
                }
            };
            let output = finder_output(rows, result.truncated);
            mcp_structured(output)
        },
    )
}

/// Keeps compact fields, overwrites `pid`/`ppid` from the typed identity, and
/// appends the opaque row-detail reference.
fn row_to_json(row: ProcessRowOut) -> Result<Value, String> {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    object.insert("pid".to_owned(), json!(row.pid));
    object.insert(
        "ppid".to_owned(),
        row.ppid.map_or(Value::Null, |ppid| json!(ppid)),
    );
    let detail_ref = kronika_query::detail_locator(
        LOGICAL_NAME,
        row.segment_id,
        row.at,
        row.type_id,
        row.row_ordinal,
        row.identity,
    )
    .detail_ref()?;
    object.retain(|field, _| !kronika_query::is_detail_text(LOGICAL_NAME, field));
    object.insert("detail_ref".to_owned(), Value::String(detail_ref));
    Ok(Value::Object(object))
}
