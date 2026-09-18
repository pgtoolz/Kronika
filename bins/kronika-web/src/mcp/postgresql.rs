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
    FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL, FIND_POSTGRESQL_INDEXES_TOOL,
    FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL, FIND_POSTGRESQL_STATEMENTS_TOOL,
    FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
};
use super::filter::{FilterInput, build_search};
use super::input::{
    ActivityInput, DatabasesInput, IndexesInput, LocksInput, PlansInput, SortInput,
    StatementsInput, TablesInput, VacuumInput,
};
use super::semantics::{bounded_limit, finder_output, mcp_error, mcp_structured};
use super::time::{TimeSpecInput, resolve_point};

// PostgreSQL finders share argument decoding and validation order. Each entry
// keeps its own input schema, tool name, query surface, and optional grouping.
macro_rules! finder_handlers {
    ($($handler:ident($input:ty, $tool:ident, $surface:ident $(, $group:ident)?);)*) => {$(
        pub(crate) fn $handler(
            config: &Config,
            arguments: Map<String, Value>,
            cancelled: &dyn Fn() -> bool,
        ) -> CallToolResult {
            let surface = FinderSurface::$surface;
            let usage = match surface {
                FinderSurface::Tables | FinderSurface::Indexes =>
                    "group and limit are required; at, filters, and sort are optional",
                _ => "limit is required; at, filters, and sort are optional",
            };
            let input: $input = match serde_json::from_value(Value::Object(arguments)) {
                Ok(input) => input,
                Err(error) => return super::semantics::invalid_arguments($tool, usage, error),
            };
            let group = None$(.or(Some(input.$group.into())))?;
            let query = match finder_query(
                $tool,
                surface,
                group,
                input.at.as_ref(),
                &input.filters,
                input.sort,
                input.limit,
            ) {
                Ok(query) => query,
                Err(error) => return error,
            };
            match (surface, group) {
                (FinderSurface::Tables, Some(group)) => call_relations(RelationKind::Tables, config, group, &query, cancelled),
                (FinderSurface::Indexes, Some(group)) => call_relations(RelationKind::Indexes, config, group, &query, cancelled),
                _ => call_plain(config, &query, cancelled),
            }
        }
    )*};
}

finder_handlers! {
    call_tables(TablesInput, FIND_POSTGRESQL_TABLES_TOOL, Tables, group);
    call_indexes(IndexesInput, FIND_POSTGRESQL_INDEXES_TOOL, Indexes, group);
    call_activity(ActivityInput, FIND_POSTGRESQL_ACTIVITY_TOOL, Activity);
    call_locks(LocksInput, FIND_POSTGRESQL_LOCKS_TOOL, Locks);
    call_vacuum(VacuumInput, FIND_POSTGRESQL_VACUUM_TOOL, Vacuum);
    call_databases(DatabasesInput, FIND_POSTGRESQL_DATABASES_TOOL, Databases);
    call_statements(StatementsInput, FIND_POSTGRESQL_STATEMENTS_TOOL, Statements);
    call_plans(PlansInput, FIND_POSTGRESQL_PLANS_TOOL, Plans);
}

fn call_relations(
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
                .map(|row| relation_row_to_json(row, kind, group))
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
fn relation_row_to_json(row: RelationRow, kind: RelationKind, group: RelationGroup) -> Value {
    let mut object = Map::new();
    for (name, metric) in row.metrics {
        object.insert(name, metric.map_or(Value::Null, |metric| metric.json()));
    }
    if let Value::Object(key_fields) = row.key.json(kind, group) {
        object.extend(key_fields);
    }
    Value::Object(object)
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

/// Keeps compact fields in mass finder output and appends its detail reference.
fn finder_plain_row_to_json(logical_name: &str, mut row: PlainRowOut) -> Result<Value, String> {
    row.fields
        .retain(|field, _value| !kronika_query::is_detail_text(logical_name, field));
    plain_row_to_json(logical_name, row)
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
