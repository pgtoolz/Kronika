//! Native-only routes around the shared recorded-data parser.

pub(crate) use kronika_api::{MAX_QUERY_BYTES, MAX_SNAPSHOT_PAGE_SIZE, RouteError};
pub(crate) use kronika_query::{Order, RelationGroup};

/// A parsed native web route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    Recorded(kronika_api::Route),
    Export(kronika_dump::SliceRange),
    McpAccess,
    InstanceLabel,
}

pub(crate) fn parse(path: &str, query: Option<&str>) -> Result<Route, RouteError> {
    let query = query.unwrap_or("");
    match path {
        "/api/export" => crate::export::parse(query).map(Route::Export),
        "/api/mcp-access" => native(query, Route::McpAccess),
        "/api/instance-label" => native(query, Route::InstanceLabel),
        _ => kronika_api::parse(path, Some(query)).map(Route::Recorded),
    }
}

fn native(query: &str, route: Route) -> Result<Route, RouteError> {
    if query.is_empty() {
        Ok(route)
    } else {
        Err(RouteError::BadParameter("query".to_owned()))
    }
}

#[cfg(test)]
mod tests;
