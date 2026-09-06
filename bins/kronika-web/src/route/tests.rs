use super::{Route, RouteError, parse};

#[test]
fn native_routes_take_no_query() {
    assert_eq!(parse("/api/mcp-access", None), Ok(Route::McpAccess));
    assert_eq!(parse("/api/instance-label", None), Ok(Route::InstanceLabel));
    for path in ["/api/mcp-access", "/api/instance-label"] {
        assert_eq!(
            parse(path, Some("verbose=1")),
            Err(RouteError::BadParameter("query".to_owned()))
        );
    }
}

#[test]
fn export_route_requires_and_parses_inclusive_seconds() {
    let Route::Export(range) = parse("/api/export", Some("from=1&to=2")).expect("export route")
    else {
        panic!("expected export route")
    };
    assert_eq!(range.from().unix_seconds(), 1);
    assert_eq!(range.to().unix_seconds(), 2);
    assert_eq!(
        parse("/api/export", Some("from=-1&to=0")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
    assert_eq!(
        parse("/api/export", None),
        Err(RouteError::BadParameter("from".to_owned()))
    );
}
