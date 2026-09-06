//! MCP adapters over recorded `PostgreSQL` finder results.

use kronika_query::RelationKind;
use kronika_query::snapshot::{
    CurrentSnapshotQuery, FinderOrder, FinderQuery, FinderResult, FinderSurface, PlainRowOut,
    RelationRow, SnapshotPoint, execute_current_plain, execute_plain, execute_relation,
};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value};

use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, RelationGroup};

use super::catalog::{
    ActivityInput, DatabasesInput, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    IndexesInput, LocksInput, PlansInput, SortInput, StatementsInput, TablesInput, VacuumInput,
};
use super::filter::{FilterInput, build_search};
use super::semantics::{bounded_limit, finder_output, mcp_error, mcp_structured};
use super::time::{TimeSpecInput, resolve_point};

pub(crate) fn call_tables(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: TablesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_TABLES_TOOL,
                "group and limit are required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let group = input.group.into();
    let query = match finder_query(
        FIND_POSTGRESQL_TABLES_TOOL,
        FinderSurface::Tables,
        Some(group),
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call(RelationKind::Tables, config, group, &query, cancelled)
}

pub(crate) fn call_indexes(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: IndexesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_INDEXES_TOOL,
                "group and limit are required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let group = input.group.into();
    let query = match finder_query(
        FIND_POSTGRESQL_INDEXES_TOOL,
        FinderSurface::Indexes,
        Some(group),
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call(RelationKind::Indexes, config, group, &query, cancelled)
}

fn call(
    kind: RelationKind,
    config: &Config,
    group: RelationGroup,
    query: &FinderQuery,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    super::run_finder_query(
        config,
        kind.logical_name(),
        |context| execute_relation(context, query, cancelled),
        |result| {
            let rows: Vec<Value> = result
                .rows
                .into_iter()
                .map(|row| row_to_json(row, kind, group))
                .collect();
            let output = finder_output(rows, result.truncated);
            mcp_structured(output)
        },
    )
}

fn finder_point(tool: &str, at: Option<&TimeSpecInput>) -> Result<SnapshotPoint, CallToolResult> {
    resolve_point(at).map_err(|error| {
        super::semantics::invalid_arguments(
            tool,
            "at is optional; group/limit and the documented finder fields keep their current shape",
            error,
        )
    })
}

fn finder_query(
    tool: &str,
    surface: FinderSurface,
    group: Option<RelationGroup>,
    at: Option<&TimeSpecInput>,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> Result<FinderQuery, CallToolResult> {
    let limit = bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE)?;
    let search = build_search(surface.logical_name(), filters)
        .map_err(super::filter::Refusal::into_error)?;
    Ok(FinderQuery {
        surface,
        point: finder_point(tool, at)?,
        search,
        order: sort.map(|sort| FinderOrder {
            field: sort.field,
            direction: sort.direction.into(),
        }),
        group,
        limit,
    })
}

/// Flattens metrics and group identity; identity fields win name collisions.
fn row_to_json(row: RelationRow, kind: RelationKind, group: RelationGroup) -> Value {
    let mut object = Map::new();
    for (name, metric) in row.metrics {
        object.insert(name, metric.map_or(Value::Null, |metric| metric.json()));
    }
    if let Value::Object(key_fields) = row.key.json(kind, group) {
        object.extend(key_fields);
    }
    Value::Object(object)
}

pub(crate) fn call_activity(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: ActivityInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_ACTIVITY_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_ACTIVITY_TOOL,
        FinderSurface::Activity,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

pub(crate) fn call_locks(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: LocksInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_LOCKS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_LOCKS_TOOL,
        FinderSurface::Locks,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

pub(crate) fn call_vacuum(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: VacuumInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_VACUUM_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_VACUUM_TOOL,
        FinderSurface::Vacuum,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

pub(crate) fn call_databases(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: DatabasesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_DATABASES_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_DATABASES_TOOL,
        FinderSurface::Databases,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

fn call_plain(
    config: &Config,
    query: &FinderQuery,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let surface = query.surface;
    super::run_finder_query(
        config,
        surface.logical_name(),
        |context| execute_plain(context, query, cancelled),
        |result| {
            let rows: Vec<Value> = match result
                .rows
                .into_iter()
                .map(|row| finder_plain_row_to_json(surface.logical_name(), row))
                .collect()
            {
                Ok(rows) => rows,
                Err(_error) => return mcp_error("could not produce detail_ref"),
            };
            let output = finder_output(rows, result.truncated);
            mcp_structured(output)
        },
    )
}

pub(super) fn plain_rows(
    logical_name: &str,
    config: &Config,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<FinderResult<PlainRowOut>>, CallToolResult> {
    let query = CurrentSnapshotQuery {
        logical_name: logical_name.to_owned(),
        fields: Vec::new(),
        order: None,
        group: None,
        limit: usize::MAX,
    };
    super::run_snapshot_query(config, |context| {
        execute_current_plain(context, query.clone(), cancelled)
    })
    .map_err(|error| super::semantics::storage_error(&error))
}

pub(crate) fn call_statements(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: StatementsInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_STATEMENTS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_STATEMENTS_TOOL,
        FinderSurface::Statements,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

pub(crate) fn call_plans(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: PlansInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_PLANS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_PLANS_TOOL,
        FinderSurface::Plans,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, &query, cancelled)
}

/// Keeps compact fields in mass finder output and appends its detail reference.
fn finder_plain_row_to_json(logical_name: &str, row: PlainRowOut) -> Result<Value, String> {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    let detail_ref = kronika_query::detail_locator(
        logical_name,
        row.segment_id,
        row.at,
        row.type_id,
        row.row_ordinal,
        row.identity,
    )
    .detail_ref()?;
    object.retain(|field, _value| !kronika_query::is_detail_text(logical_name, field));
    object.insert("detail_ref".to_owned(), Value::String(detail_ref));
    Ok(Value::Object(object))
}

/// Flattens projected fields and appends the shared opaque detail reference.
pub(super) fn plain_row_to_json(logical_name: &str, row: PlainRowOut) -> Result<Value, String> {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    let detail_ref = kronika_query::detail_locator(
        logical_name,
        row.segment_id,
        row.at,
        row.type_id,
        row.row_ordinal,
        row.identity,
    )
    .detail_ref()?;
    object.insert("detail_ref".to_owned(), Value::String(detail_ref));
    Ok(Value::Object(object))
}
