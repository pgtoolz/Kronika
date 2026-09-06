use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorData};

use crate::config::Config;

use super::catalog::{
    FIND_EVENTS_TOOL, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    FIND_PROCESSES_TOOL, GET_CONTEXT_TOOL, GET_INSTANCE_TOOL, GET_ROW_DETAIL_TOOL, OVERVIEW_TOOL,
};
use super::{
    context, events, filter, instance, overview, postgresql, processes, row_detail, semantics,
};

/// An unknown tool name is a protocol-level error, not a tool result:
/// `isError: true` is reserved for execution after a valid tool was
/// selected, so a client can tell a stale tool name from a failing call.
pub(crate) fn dispatch(
    config: &Config,
    request: CallToolRequestParams,
    cancelled: &dyn Fn() -> bool,
) -> Result<CallToolResult, ErrorData> {
    let arguments = request.arguments.unwrap_or_default();
    let finder_section = match request.name.as_ref() {
        FIND_PROCESSES_TOOL => Some("os_process"),
        FIND_POSTGRESQL_TABLES_TOOL => Some("pg_stat_user_tables"),
        FIND_POSTGRESQL_INDEXES_TOOL => Some("pg_stat_user_indexes"),
        FIND_POSTGRESQL_ACTIVITY_TOOL => Some("pg_stat_activity"),
        FIND_POSTGRESQL_LOCKS_TOOL => Some("pg_locks"),
        FIND_POSTGRESQL_VACUUM_TOOL => Some("pg_stat_progress_vacuum"),
        FIND_POSTGRESQL_DATABASES_TOOL => Some("pg_stat_database"),
        FIND_POSTGRESQL_STATEMENTS_TOOL => Some("pg_stat_statements"),
        FIND_POSTGRESQL_PLANS_TOOL => Some("pg_store_plans"),
        _ => None,
    };
    if finder_section.is_some() && !semantics::arguments_within_budget(&arguments) {
        return Ok(semantics::mcp_error(format!(
            "{} arguments exceed {} encoded bytes; narrow filters or time input",
            request.name,
            crate::route::MAX_QUERY_BYTES
        )));
    }
    if let Some(logical_name) = finder_section
        && let Err(error) = filter::validate_filter_operators(logical_name, &arguments)
    {
        return Ok(error);
    }
    Ok(match request.name.as_ref() {
        GET_CONTEXT_TOOL => context::call(config, arguments, cancelled),
        GET_INSTANCE_TOOL => instance::call(config, arguments, cancelled),
        OVERVIEW_TOOL => overview::call(config, arguments, cancelled),
        FIND_POSTGRESQL_TABLES_TOOL => postgresql::call_tables(config, arguments, cancelled),
        FIND_POSTGRESQL_INDEXES_TOOL => postgresql::call_indexes(config, arguments, cancelled),
        FIND_POSTGRESQL_ACTIVITY_TOOL => postgresql::call_activity(config, arguments, cancelled),
        FIND_POSTGRESQL_LOCKS_TOOL => postgresql::call_locks(config, arguments, cancelled),
        FIND_POSTGRESQL_VACUUM_TOOL => postgresql::call_vacuum(config, arguments, cancelled),
        FIND_POSTGRESQL_DATABASES_TOOL => postgresql::call_databases(config, arguments, cancelled),
        FIND_POSTGRESQL_STATEMENTS_TOOL => {
            postgresql::call_statements(config, arguments, cancelled)
        }
        FIND_POSTGRESQL_PLANS_TOOL => postgresql::call_plans(config, arguments, cancelled),
        FIND_PROCESSES_TOOL => processes::call(config, arguments, cancelled),
        GET_ROW_DETAIL_TOOL => row_detail::call(config, arguments, cancelled),
        FIND_EVENTS_TOOL => events::call(config, arguments, cancelled),
        other => {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {other}"),
                None,
            ));
        }
    })
}
