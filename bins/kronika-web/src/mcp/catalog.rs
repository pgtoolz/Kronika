//! Ordered tool names, descriptions, and schemas returned by `tools/list`.

use rmcp::model::Tool;
use schemars::JsonSchema;

use super::input::{
    ActivityInput, DatabasesInput, EventsInput, GetContextInput, GetInstanceInput, IndexesInput,
    LocksInput, OverviewInput, PlansInput, ProcessesInput, RowDetailInput, StatementsInput,
    TablesInput, VacuumInput,
};
use super::instance::InstanceOutput;
use super::schema::{
    EventsResult, HeatmapBatchResult, RecordedSectionsOutput, opaque_output_schema,
    output_schema_object, row_detail_output_schema, schema_object,
};
use super::semantics::FinderOutput;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_rank_metrics";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_list_recorded_sections";
pub(crate) const GET_INSTANCE_TOOL: &str = "kronika_get_instance";
pub(crate) const FIND_POSTGRESQL_TABLES_TOOL: &str = "kronika_find_postgresql_tables";
pub(crate) const FIND_POSTGRESQL_INDEXES_TOOL: &str = "kronika_find_postgresql_indexes";
pub(crate) const FIND_POSTGRESQL_ACTIVITY_TOOL: &str = "kronika_find_postgresql_activity";
pub(crate) const FIND_POSTGRESQL_LOCKS_TOOL: &str = "kronika_find_postgresql_locks";
pub(crate) const FIND_POSTGRESQL_VACUUM_TOOL: &str = "kronika_find_postgresql_vacuum";
pub(crate) const FIND_POSTGRESQL_DATABASES_TOOL: &str = "kronika_find_postgresql_databases";
pub(crate) const FIND_POSTGRESQL_STATEMENTS_TOOL: &str = "kronika_find_postgresql_statements";
pub(crate) const FIND_POSTGRESQL_PLANS_TOOL: &str = "kronika_find_postgresql_plans";
pub(crate) const FIND_PROCESSES_TOOL: &str = "kronika_find_processes";
pub(crate) const GET_ROW_DETAIL_TOOL: &str = "kronika_get_row_detail";
pub(crate) const FIND_EVENTS_TOOL: &str = "kronika_find_events";

pub(crate) const SERVER_INSTRUCTIONS: &str = "Kronika returns observability data that it has already recorded. It never reads current state directly from a host or database.\n\n`kronika_list_recorded_sections` lists the recorded time bounds and the available sections, fields, units, and sources. Finder tools read one recorded point in time. `kronika_rank_metrics` and `kronika_find_events` read the half-open interval `[from,to)`.\n\n`now` is resolved when the request is handled and does not indicate the timestamp of the newest recorded observation.\n\nA `detail_ref` is opaque. Pass it unchanged to `kronika_get_row_detail`.";

pub(crate) fn tools() -> Vec<Tool> {
    [overview_tool(), context_tool(), instance_tool()]
        .into_iter()
        .chain(relation_tools())
        .chain(postgresql_plain_tools())
        .chain(statement_plan_tools())
        .chain([processes_tool(), row_detail_tool(), events_tool()])
        .collect()
}

fn overview_tool() -> Tool {
    Tool::new(
        OVERVIEW_TOOL,
        "Ranks recorded numeric fields over the half-open interval `[from,to)`. \
         Each requested field produces a separate result in request order. Counter \
         totals are non-negative changes across the interval; gauge totals are \
         maximum recorded values. Use `kronika_list_recorded_sections` when a \
         section or field name is unknown.",
        schema_object::<OverviewInput>(),
    )
    .with_raw_output_schema(opaque_output_schema::<HeatmapBatchResult>())
}

fn context_tool() -> Tool {
    Tool::new(
        GET_CONTEXT_TOOL,
        "Lists the recorded time bounds and sections available in Kronika. Each \
         section includes its source, row and byte counts, and public fields with \
         their class and unit. Pass `section` to return one section.",
        schema_object::<GetContextInput>(),
    )
    .with_raw_output_schema(output_schema_object::<RecordedSectionsOutput>())
}

/// Instance metadata tool.
fn instance_tool() -> Tool {
    Tool::new(
        GET_INSTANCE_TOOL,
        "Returns the latest recorded host metadata and PostgreSQL settings. Host \
         metadata and settings have separate recorded timestamps. Settings whose \
         recorded source is `default` are omitted unless `settings` is `\"all\"`.",
        schema_object::<GetInstanceInput>(),
    )
    .with_raw_output_schema(output_schema_object::<InstanceOutput>())
}

/// `PostgreSQL` relation tools.
fn relation_tools() -> [Tool; 2] {
    [
        finder_tool::<TablesInput>(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Finds or groups recorded PostgreSQL table statistics at `at`. Filters \
             are applied before aggregation. `group` selects table, database, schema, \
             or tablespace; each metric uses the reducer stated in its field \
             description. Aggregated rows do not have `detail_ref`.",
        ),
        finder_tool::<IndexesInput>(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Finds or groups recorded PostgreSQL index statistics at `at`. Filters \
             are applied before aggregation. `group` selects index, database, schema, \
             or tablespace; each metric uses the reducer stated in its field \
             description. Aggregated rows do not have `detail_ref`.",
        ),
    ]
}

fn processes_tool() -> Tool {
    finder_tool::<ProcessesInput>(
        FIND_PROCESSES_TOOL,
        "Finds Linux process observations at `at`, then applies filters, sorting, \
             and `limit`. Rates are derived between compatible observations of the \
             same process; unavailable rates are null. Command lines are available \
             through `detail_ref`.",
    )
}

fn row_detail_tool() -> Tool {
    Tool::new(
        GET_ROW_DETAIL_TOOL,
        "Returns the stored row addressed by `detail_ref`. Pass a reference \
             emitted by Kronika unchanged. Long text is returned as \
             `{stored_text, full_len, truncated, sha256}`.",
        schema_object::<RowDetailInput>(),
    )
    .with_raw_output_schema(row_detail_output_schema())
}

fn events_tool() -> Tool {
    Tool::new(
        FIND_EVENTS_TOOL,
        "Finds recorded PostgreSQL and PgBouncer events in the half-open \
             interval `[from,to)`. `groups` returns merged summaries; `occurrences` \
             returns individual events. `limit` is applied after merging or grouping. \
             Raw event payloads are available through `detail_ref`.",
        schema_object::<EventsInput>(),
    )
    .with_raw_output_schema(opaque_output_schema::<EventsResult>())
}

/// `PostgreSQL` point finders.
fn postgresql_plain_tools() -> [Tool; 4] {
    [
        finder_tool::<ActivityInput>(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Finds PostgreSQL backend activity at `at`, including state, waits, \
             query identifiers, and transaction or query timestamps. Query text is \
             available through `detail_ref`.",
        ),
        finder_tool::<LocksInput>(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Finds PostgreSQL backends participating in direct lock waits at `at`. \
             `blocked_by` contains direct blocker PIDs; an empty list marks a root \
             or blocker-only row, and PID `0` denotes a prepared transaction.",
        ),
        finder_tool::<VacuumInput>(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Finds PostgreSQL backends recorded as running `VACUUM` near `at`, \
             including progress counts, dead-tuple storage, and delay time. An empty \
             result means no vacuum observation was selected around that point.",
        ),
        finder_tool::<DatabasesInput>(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Finds recorded PostgreSQL statistics for each database at `at`. \
             Cumulative counts, bytes, and times are returned as interval rates; \
             `numbackends` is a recorded count. Missing predecessors and counter \
             resets produce null rates.",
        ),
    ]
}

/// `PostgreSQL` statement and plan point finders.
fn statement_plan_tools() -> [Tool; 2] {
    [
        finder_tool::<StatementsInput>(
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            "Finds recorded `pg_stat_statements` rows at `at`. Cumulative values \
             are returned as interval rates; latency statistics remain recorded \
             gauges. Derived fields expose per-call values and fractions defined in \
             their field descriptions. Query text is available through `detail_ref`.",
        ),
        finder_tool::<PlansInput>(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Finds recorded `pg_store_plans` rows at `at`. `calls` is the recorded \
             cumulative count and `calls_per_second` is its interval rate; other \
             cumulative values are returned as rates. Plan text is available through \
             `detail_ref`.",
        ),
    ]
}

fn finder_tool<I: JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, schema_object::<I>())
        .with_raw_output_schema(output_schema_object::<FinderOutput>())
}

#[cfg(test)]
#[path = "../tests/mcp/catalog.rs"]
mod tests;
