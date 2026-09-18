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
fn native_routes_accept_only_build_cache_metadata() {
    for (path, route) in [
        ("/api/mcp-access", Route::McpAccess),
        ("/api/instance-label", Route::InstanceLabel),
    ] {
        assert_eq!(parse(path, Some("build=84e1609")), Ok(route));
        for query in [
            "build=84e1609&verbose=1",
            "build=84e1609&",
            "&build=84e1609",
        ] {
            assert_eq!(
                parse(path, Some(query)),
                Err(RouteError::BadParameter("query".to_owned())),
                "{path}?{query}"
            );
        }
        assert!(
            parse(
                path,
                Some(&format!("build={}", "x".repeat(super::MAX_QUERY_BYTES)))
            )
            .is_err()
        );
    }
}

#[test]
fn export_build_cache_metadata_preserves_range_validation() {
    let Route::Export(range) = parse("/api/export", Some("from=1&build=84e1609&to=2"))
        .expect("build metadata does not change the range")
    else {
        panic!("expected export route")
    };
    assert_eq!(range.from().unix_seconds(), 1);
    assert_eq!(range.to().unix_seconds(), 2);
    for (query, parameter) in [
        ("build=84e1609&from=1&from=2&to=3", "from"),
        ("build=84e1609&from=1&to=2&to=3", "to"),
        ("build=84e1609&from=1&to=2&verbose=1", "verbose"),
        ("build=84e1609&from=1&to=2&", "query"),
        ("build=84e1609", "from"),
    ] {
        assert_eq!(
            parse("/api/export", Some(query)),
            Err(RouteError::BadParameter(parameter.to_owned())),
            "{query}"
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
