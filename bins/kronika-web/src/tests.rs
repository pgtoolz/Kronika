use http_body_util::BodyExt as _;
use hyper::header::{
    ACCEPT_ENCODING, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, COOKIE, ETAG, IF_NONE_MATCH, ORIGIN,
    SET_COOKIE, VARY, WWW_AUTHENTICATE,
};
use hyper::{Method, Request, StatusCode};
use tokio::sync::{mpsc, oneshot};

use super::{
    RequestError, RequestTarget, SessionTarget, SingleHeader, authorization, if_none_match_values,
    response_from_meta, route_request, route_request_at, route_request_without_authentication,
    session_response,
};
use crate::api::{ApiError, CachePolicy, Prepared, ResponseMeta};
use crate::body::StreamHead;
use crate::config::Account;
use crate::encoding::{AcceptedEncodings, ContentCoding};

pub(crate) mod artifacts;
mod multi_layout;

const AUTHORIZATION: &str = "Basic ZGJhOnNlY3JldA==";

fn account() -> Account {
    Account {
        user: "dba".to_owned(),
        password: "secret".to_owned(),
    }
}

fn request(method: Method, target: &str) -> Request<()> {
    Request::builder()
        .method(method)
        .uri(target)
        .header(hyper::header::AUTHORIZATION, AUTHORIZATION)
        .body(())
        .expect("request")
}

fn public_request(method: Method, target: &str) -> Request<()> {
    Request::builder()
        .method(method)
        .uri(target)
        .body(())
        .expect("request")
}

fn session_request(
    method: Method,
    authorization: Option<&str>,
    cookie: Option<&str>,
) -> Request<()> {
    let mut request = public_request(method, "/auth/session");
    if let Some(authorization) = authorization {
        request.headers_mut().insert(
            hyper::header::AUTHORIZATION,
            hyper::header::HeaderValue::from_str(authorization).expect("authorization header"),
        );
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            COOKIE,
            hyper::header::HeaderValue::from_str(cookie).expect("cookie header"),
        );
    }
    request
        .headers_mut()
        .insert("x-kronika-ui", hyper::header::HeaderValue::from_static("1"));
    request
}

fn session_request_from_origin(
    method: Method,
    authorization: Option<&str>,
    cookie: Option<&str>,
    origin: &'static str,
) -> Request<()> {
    session_request_from_origins(method, authorization, cookie, &[origin])
}

fn session_request_from_origins(
    method: Method,
    authorization: Option<&str>,
    cookie: Option<&str>,
    origins: &[&'static str],
) -> Request<()> {
    let mut request = session_request(method, authorization, cookie);
    for origin in origins {
        request
            .headers_mut()
            .append(ORIGIN, hyper::header::HeaderValue::from_static(origin));
    }
    request
}

fn session_route_response(request: &Request<()>, now: u64) -> hyper::Response<super::WebBody> {
    match route_request_at(&account(), request, now).expect("session route") {
        RequestTarget::Session(target) => {
            session_response(&account(), target).expect("session response")
        }
        other => panic!("expected session target, got {other:?}"),
    }
}

#[test]
fn the_instance_label_names_the_largest_recorded_database() {
    let mut fixture = artifacts::Fixture::new();
    fixture.append_placed_table_snapshots(&[
        (
            100, 1, 11, 0, "small", "public", "t1", None, None, 100, None,
        ),
        (100, 2, 21, 0, "big", "public", "t2", None, None, 900, None),
    ]);
    fixture.finish();
    let config = |root: &std::path::Path| crate::config::Config {
        data_root: root.to_path_buf(),
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: account(),
        authentication_required: true,
        sources: crate::config::SOURCE_POSTGRESQL,
        synthetic_demo: false,
        export_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    };
    assert_eq!(
        crate::largest_database(&config(fixture.root())),
        Some("big".to_owned())
    );
    let empty = artifacts::Fixture::new();
    assert_eq!(crate::largest_database(&config(empty.root())), None);

    let body: serde_json::Value =
        serde_json::from_str(&crate::instance_label_body(Some("big"))).expect("json");
    assert_eq!(body["record"], "instance_label");
    assert_eq!(body["database"], "big");
    let body: serde_json::Value =
        serde_json::from_str(&crate::instance_label_body(None)).expect("json");
    assert!(body["database"].is_null());
}

#[test]
fn the_day_cache_header_overrides_no_store() {
    let cached = crate::day_cached_private(crate::json_response(StatusCode::OK, "{}".to_owned()));
    assert_eq!(
        cached.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static(
            "private,max-age=86400"
        ))
    );
    assert_eq!(
        cached.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Cookie"
        ))
    );
    let plain = crate::json_response(StatusCode::OK, "{}".to_owned());
    assert_eq!(
        plain.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("private,no-store"))
    );
}

fn request_cookie(set_cookie: &str) -> &str {
    set_cookie.split(';').next().expect("request cookie")
}

fn rejection(method: Method, target: &str) -> hyper::Response<super::WebBody> {
    route_request(&account(), &request(method, target))
        .expect_err("request is rejected")
        .response()
}

#[test]
fn authentication_is_mandatory_and_central() {
    for target in [
        "/api",
        "/api/",
        "/api/catalog",
        "/api/export?from=1&to=2",
        "/api/mcp-access",
        "/api/instance-label",
        "/api/not-a-resource",
    ] {
        let response = route_request(&account(), &public_request(Method::GET, target))
            .expect_err("missing credentials")
            .response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{target}");
        assert_eq!(
            response.headers().get(WWW_AUTHENTICATE),
            Some(&hyper::header::HeaderValue::from_static(
                "Basic realm=\"kronika\""
            )),
            "{target}"
        );
        assert_eq!(response.headers().get(VARY), Some(&unauthorized_vary()));
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
    }
}

#[test]
fn the_shell_is_public_even_with_invalid_authorization() {
    let root = public_request(Method::GET, "/");
    assert_eq!(
        route_request(&account(), &root),
        Ok(RequestTarget::Ui {
            head: false,
            coding: ContentCoding::Identity,
        })
    );

    let mut index = public_request(Method::HEAD, "/index.html");
    index.headers_mut().insert(
        hyper::header::AUTHORIZATION,
        hyper::header::HeaderValue::from_static("Basic invalid"),
    );
    assert_eq!(
        route_request(&account(), &index),
        Ok(RequestTarget::Ui {
            head: true,
            coding: ContentCoding::Identity,
        })
    );
}

#[test]
fn public_non_api_paths_are_not_found() {
    for target in ["/health", "/index.html/", "/api-not-a-resource"] {
        let response = route_request(&account(), &public_request(Method::GET, target))
            .expect_err("unknown public path")
            .response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{target}");
        assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    }
}

#[test]
fn duplicate_authorization_fields_are_refused() {
    let mut request = request(Method::GET, "/api/catalog");
    request.headers_mut().append(
        hyper::header::AUTHORIZATION,
        hyper::header::HeaderValue::from_static(AUTHORIZATION),
    );
    assert_eq!(authorization(request.headers()), SingleHeader::Invalid);
    assert_eq!(
        route_request(&account(), &request)
            .expect_err("ambiguous credentials")
            .response()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn session_get_is_query_free_empty_and_unchallenged() {
    const NOW: u64 = 1_786_579_200;
    let response = session_route_response(&session_request(Method::GET, None, None), NOW);
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("no-store"))
    );
    assert_eq!(response.headers().get(VARY), Some(&unauthorized_vary()));
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    assert!(!response.headers().contains_key(SET_COOKIE));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("session body")
            .to_bytes()
            .is_empty()
    );

    let queried = public_request(Method::GET, "/auth/session?next=/");
    assert_eq!(
        route_request_at(&account(), &queried, NOW)
            .expect_err("query is not a session route")
            .response()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn session_post_with_http_origin_issues_a_credential_free_cookie() {
    const NOW: u64 = 1_786_579_200;
    let mut request = session_request_from_origin(
        Method::POST,
        Some(AUTHORIZATION),
        None,
        "http://kronika.example",
    );
    request.headers_mut().insert(
        "x-forwarded-proto",
        hyper::header::HeaderValue::from_static("https"),
    );
    request.headers_mut().insert(
        "forwarded",
        hyper::header::HeaderValue::from_static("for=192.0.2.1;proto=https"),
    );
    let response = session_route_response(&request, NOW);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("no-store"))
    );
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .expect("session cookie")
        .to_str()
        .expect("ASCII cookie");
    assert!(cookie.starts_with("kronika_session=v1."));
    assert!(!cookie.contains("dba"));
    assert!(!cookie.contains("secret"));
    assert!(!cookie.contains("; Secure"));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("session body")
            .to_bytes()
            .is_empty()
    );
}

#[test]
fn session_post_with_https_origin_issues_a_secure_cookie() {
    const NOW: u64 = 1_786_579_200;
    let request = session_request_from_origin(
        Method::POST,
        Some(AUTHORIZATION),
        None,
        "https://kronika.example",
    );
    let response = session_route_response(&request, NOW);
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("session cookie");

    assert!(cookie.ends_with("; Secure"));
}

#[test]
fn session_cookie_mutations_require_one_well_formed_http_origin() {
    const NOW: u64 = 1_786_579_200;
    let invalid_origins: [&[&str]; 5] = [
        &[],
        &["null"],
        &["ftp://kronika.example"],
        &["https://kronika.example/path"],
        &["https://kronika.example", "https://kronika.example"],
    ];
    for method in [Method::POST, Method::DELETE] {
        let authorization = (method == Method::POST).then_some(AUTHORIZATION);
        for origins in invalid_origins {
            let request =
                session_request_from_origins(method.clone(), authorization, None, origins);
            assert_eq!(
                route_request_at(&account(), &request, NOW),
                Err(RequestError::InvalidOrigin),
                "{method} {origins:?}"
            );
        }
    }
}

#[test]
fn invalid_session_post_is_unchallenged_and_does_not_change_the_cookie() {
    const NOW: u64 = 1_786_579_200;
    let response = session_route_response(
        &session_request(Method::POST, Some("Basic ZGJhOndyb25n"), None),
        NOW,
    );
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    assert!(!response.headers().contains_key(SET_COOKIE));
}

#[test]
fn disabled_auth_session_post_remains_admitted_without_an_origin() {
    let request = session_request(Method::POST, None, None);
    assert_eq!(
        route_request_without_authentication(&request),
        Ok(RequestTarget::Session(SessionTarget::Check {
            admitted: true,
        }))
    );
}

#[test]
fn duplicate_authorization_cannot_create_a_session() {
    const NOW: u64 = 1_786_579_200;
    let mut request = session_request(Method::POST, Some(AUTHORIZATION), None);
    request.headers_mut().append(
        hyper::header::AUTHORIZATION,
        hyper::header::HeaderValue::from_static(AUTHORIZATION),
    );
    let response = session_route_response(&request, NOW);
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(SET_COOKIE));
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
}

#[test]
fn valid_cookie_restores_session_and_authorizes_browser_api() {
    const NOW: u64 = 1_786_579_200;
    let login = session_route_response(
        &session_request_from_origin(
            Method::POST,
            Some(AUTHORIZATION),
            None,
            "http://kronika.example",
        ),
        NOW,
    );
    let cookie = request_cookie(
        login
            .headers()
            .get(SET_COOKIE)
            .expect("session cookie")
            .to_str()
            .expect("ASCII cookie"),
    );

    let get = session_route_response(&session_request(Method::GET, None, Some(cookie)), NOW);
    assert_eq!(get.status(), StatusCode::NO_CONTENT);
    assert!(!get.headers().contains_key(SET_COOKIE));

    let mut api = public_request(Method::GET, "/api/catalog");
    api.headers_mut().insert(
        COOKIE,
        hyper::header::HeaderValue::from_str(cookie).expect("cookie header"),
    );
    api.headers_mut()
        .insert("x-kronika-ui", hyper::header::HeaderValue::from_static("1"));
    assert!(matches!(
        route_request_at(&account(), &api, NOW),
        Ok(RequestTarget::Api { .. })
    ));
}

#[test]
fn expired_cookie_is_rejected_without_implicit_cleanup() {
    const NOW: u64 = 1_786_579_200;
    let issued = crate::auth::issue_cookie(&account(), NOW, false);
    let cookie = request_cookie(&issued);
    let expired_at = NOW + crate::auth::SESSION_MAX_AGE;

    let get = session_route_response(
        &session_request(Method::GET, None, Some(cookie)),
        expired_at,
    );
    assert_eq!(get.status(), StatusCode::UNAUTHORIZED);
    assert!(!get.headers().contains_key(SET_COOKIE));

    let mut api = public_request(Method::GET, "/api/catalog");
    api.headers_mut().insert(
        COOKIE,
        hyper::header::HeaderValue::from_str(cookie).expect("cookie header"),
    );
    api.headers_mut()
        .insert("x-kronika-ui", hyper::header::HeaderValue::from_static("1"));
    let response = route_request_at(&account(), &api, expired_at)
        .expect_err("expired session")
        .response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(SET_COOKIE));
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
}

#[test]
fn mcp_request_with_a_well_formed_origin_is_rejected() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(ORIGIN, "https://example.com")
        .body(())
        .expect("request");
    let target = route_request_at(&account(), &request, 0);
    assert!(matches!(target, Err(RequestError::OriginNotAllowed)));
}

#[test]
fn mcp_request_with_no_origin_and_valid_auth_reaches_the_mcp_target() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(hyper::header::AUTHORIZATION, AUTHORIZATION)
        .body(())
        .expect("request");
    let target = route_request_at(&account(), &request, 0);
    assert!(matches!(target, Ok(RequestTarget::Mcp)));
}

#[test]
fn direct_basic_api_remains_available() {
    const NOW: u64 = 1_786_579_200;
    assert!(matches!(
        route_request_at(&account(), &request(Method::GET, "/api/catalog"), NOW),
        Ok(RequestTarget::Api { .. })
    ));
}

#[test]
fn marked_browser_api_unauthorized_is_silent_and_does_not_clear() {
    const NOW: u64 = 1_786_579_200;
    let mut request = public_request(Method::GET, "/api/catalog");
    request
        .headers_mut()
        .insert("x-kronika-ui", hyper::header::HeaderValue::from_static("1"));
    let response = route_request_at(&account(), &request, NOW)
        .expect_err("missing browser session")
        .response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    assert!(!response.headers().contains_key(SET_COOKIE));
}

#[test]
fn malformed_or_duplicate_authorization_does_not_fall_back_to_cookie() {
    const NOW: u64 = 1_786_579_200;
    let issued = crate::auth::issue_cookie(&account(), NOW, false);
    let cookie = request_cookie(&issued);

    for duplicate in [false, true] {
        let mut request = public_request(Method::GET, "/api/catalog");
        request.headers_mut().insert(
            COOKIE,
            hyper::header::HeaderValue::from_str(cookie).expect("cookie header"),
        );
        request.headers_mut().insert(
            hyper::header::AUTHORIZATION,
            hyper::header::HeaderValue::from_static("Basic invalid"),
        );
        request
            .headers_mut()
            .insert("x-kronika-ui", hyper::header::HeaderValue::from_static("1"));
        if duplicate {
            request.headers_mut().append(
                hyper::header::AUTHORIZATION,
                hyper::header::HeaderValue::from_static(AUTHORIZATION),
            );
        }
        let response = route_request_at(&account(), &request, NOW)
            .expect_err("authorization field takes precedence")
            .response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{duplicate}");
        assert!(!response.headers().contains_key(SET_COOKIE));
    }
}

#[test]
fn duplicate_cookie_fields_are_not_a_session() {
    const NOW: u64 = 1_786_579_200;
    let issued = crate::auth::issue_cookie(&account(), NOW, false);
    let cookie = request_cookie(&issued);
    let mut request = session_request(Method::GET, None, Some(cookie));
    request.headers_mut().append(
        COOKIE,
        hyper::header::HeaderValue::from_str(cookie).expect("cookie header"),
    );
    let response = session_route_response(&request, NOW);
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!response.headers().contains_key(SET_COOKIE));
}

#[tokio::test]
async fn session_delete_is_idempotent_and_clears_the_exact_scope() {
    const NOW: u64 = 1_786_579_200;
    let response = session_route_response(
        &session_request_from_origin(Method::DELETE, None, None, "https://kronika.example"),
        NOW,
    );
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok()),
        Some("kronika_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; Secure")
    );
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("session body")
            .to_bytes()
            .is_empty()
    );
}

#[test]
fn session_delete_with_http_origin_clears_without_secure() {
    const NOW: u64 = 1_786_579_200;
    let response = session_route_response(
        &session_request_from_origin(Method::DELETE, None, None, "http://kronika.example"),
        NOW,
    );
    assert_eq!(
        response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok()),
        Some("kronika_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    );
}

#[tokio::test]
async fn unsupported_session_method_stays_inside_the_empty_session_boundary() {
    const NOW: u64 = 1_786_579_200;
    let response = session_route_response(&session_request(Method::PUT, None, None), NOW);
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static(
            "GET, POST, DELETE"
        ))
    );
    assert_eq!(
        response.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("no-store"))
    );
    assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("session body")
            .to_bytes()
            .is_empty()
    );
}

#[test]
fn route_recognition_precedes_the_method_check() {
    let unknown = rejection(Method::POST, "/api/not-a-resource");
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let known = rejection(Method::POST, "/api/catalog");
    assert_eq!(known.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        known.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET"))
    );

    let export = rejection(Method::POST, "/api/export?from=1&to=2");
    assert_eq!(export.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        export.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET"))
    );

    let api_head = rejection(Method::HEAD, "/api/catalog");
    assert_eq!(api_head.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        api_head.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET"))
    );
}

#[test]
fn only_the_two_exact_ui_paths_admit_get_and_head() {
    assert_eq!(
        route_request(&account(), &request(Method::GET, "/")),
        Ok(RequestTarget::Ui {
            head: false,
            coding: ContentCoding::Identity,
        })
    );
    assert_eq!(
        route_request(&account(), &request(Method::HEAD, "/index.html")),
        Ok(RequestTarget::Ui {
            head: true,
            coding: ContentCoding::Identity,
        })
    );
    assert!(matches!(
        route_request(&account(), &request(Method::GET, "/api/catalog")),
        Ok(RequestTarget::Api {
            route: crate::route::Route::Recorded(kronika_api::Route::Catalog(_)),
            ..
        })
    ));

    let post = route_request(&account(), &public_request(Method::POST, "/"))
        .expect_err("shell POST is rejected")
        .response();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        post.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET, HEAD"))
    );
    assert_eq!(
        rejection(Method::GET, "/index.html/").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rejection(Method::GET, "/api/not-a-resource").status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn a_ui_client_that_refuses_every_representation_gets_an_explicit_406() {
    for value in ["identity;q=0", "*;q=0", "gzip;q=0, identity;q=0"] {
        let mut request = request(Method::GET, "/");
        request.headers_mut().insert(
            ACCEPT_ENCODING,
            hyper::header::HeaderValue::from_static(value),
        );
        let response = route_request(&account(), &request)
            .expect_err("UI request refuses both representations")
            .response();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{value}");
        assert_eq!(
            response.headers().get(VARY),
            Some(&hyper::header::HeaderValue::from_static(
                "Authorization, Accept-Encoding"
            )),
            "{value}"
        );
    }
}

#[tokio::test]
async fn an_ordinary_curl_request_receives_readable_identity_html() {
    let target =
        route_request(&account(), &public_request(Method::GET, "/")).expect("ordinary curl route");
    let RequestTarget::Ui { head, coding } = target else {
        panic!("ordinary curl must select the UI")
    };
    assert!(!head);
    assert_eq!(coding, ContentCoding::Identity);

    let response = crate::ui::response(head, None, coding).expect("identity response");
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("identity body")
        .to_bytes();
    assert!(body.starts_with(b"<!doctype html>"));
    assert_ne!(body.get(..2), Some([0x1f, 0x8b].as_slice()));
}

#[test]
fn an_api_client_that_refuses_every_coding_gets_an_explicit_406() {
    let mut request = request(Method::GET, "/api/catalog");
    request.headers_mut().insert(
        ACCEPT_ENCODING,
        hyper::header::HeaderValue::from_static("gzip;q=0, identity;q=0"),
    );
    let response = route_request(&account(), &request)
        .expect_err("no API representation is acceptable")
        .response();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
    );
}

#[test]
fn an_export_client_must_accept_the_identity_artifact() {
    let mut request = request(Method::GET, "/api/export?from=1&to=2");
    request.headers_mut().insert(
        ACCEPT_ENCODING,
        hyper::header::HeaderValue::from_static("gzip, identity;q=0"),
    );
    let response = route_request(&account(), &request)
        .expect_err("an HTML export is never content encoded")
        .response();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
    );
}

#[test]
fn all_if_none_match_fields_reach_the_matcher() {
    let mut request = request(Method::GET, "/api/segments/7/sections/os_cpu/index");
    request.headers_mut().append(
        IF_NONE_MATCH,
        hyper::header::HeaderValue::from_static("\"first\""),
    );
    request.headers_mut().append(
        IF_NONE_MATCH,
        hyper::header::HeaderValue::from_static("W/\"second\", \"third\""),
    );
    assert_eq!(
        if_none_match_values(request.headers()).as_deref(),
        Some("\"first\",W/\"second\", \"third\"")
    );
}

#[tokio::test]
async fn response_metadata_controls_private_cache_headers_etag_and_vary() {
    let (_sender, receiver) = mpsc::channel(1);
    let response = response_from_meta(
        StreamHead {
            meta: ResponseMeta {
                status: StatusCode::NOT_MODIFIED,
                cache: CachePolicy::Revalidate,
                etag: Some("W/\"1234abcd\"".to_owned()),
            },
            coding: None,
        },
        receiver,
    );
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        response.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("private,no-cache"))
    );
    assert_eq!(
        response.headers().get(ETAG),
        Some(&hyper::header::HeaderValue::from_static("W/\"1234abcd\""))
    );
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
    );
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("304 body")
            .to_bytes()
            .is_empty()
    );
}

const fn unauthorized_vary() -> hyper::header::HeaderValue {
    hyper::header::HeaderValue::from_static("Authorization, Cookie, X-Kronika-UI")
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_resource_work_does_not_stall_the_current_thread_runtime() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let response = tokio::spawn(super::blocking_stream(
        move || {
            let _sent = entered_tx.send(());
            release_rx.recv().expect("release blocking producer");
            Ok(Prepared::Empty(ResponseMeta {
                status: StatusCode::OK,
                cache: CachePolicy::NoStore,
                etag: None,
            }))
        },
        AcceptedEncodings::default(),
    ));

    entered_rx.await.expect("producer entered blocking pool");
    let (serviced_tx, serviced_rx) = oneshot::channel();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        let _sent = serviced_tx.send(());
    });
    serviced_rx
        .await
        .expect("unrelated future runs while producer is blocked");
    release_tx.send(()).expect("release producer");

    assert_eq!(
        response.await.expect("response task").status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn changed_journal_generation_replays_preparation_once() {
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&attempts);
    let response = super::blocking_stream_with_replay(
        move || {
            if observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                return Err(ApiError::Unreadable(Box::new(
                    kronika_reader::ReaderError::Io(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "active part belongs to another journal generation",
                    )),
                )));
            }
            Ok(Prepared::Empty(ResponseMeta {
                status: StatusCode::OK,
                cache: CachePolicy::NoStore,
                etag: None,
            }))
        },
        AcceptedEncodings::default(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn source_change_detection_reaches_reader_errors_inside_index_wrappers() {
    let wrapped = ApiError::Unreadable(Box::new(kronika_index::LoadError::Build(
        kronika_index::BuildError::Reader(kronika_reader::ReaderError::Io(std::io::Error::from(
            std::io::ErrorKind::Interrupted,
        ))),
    )));
    assert!(wrapped.source_changed_during_read());

    let unrelated = ApiError::Unreadable(Box::new(std::io::Error::from(
        std::io::ErrorKind::UnexpectedEof,
    )));
    assert!(!unrelated.source_changed_during_read());
}

#[tokio::test]
async fn changed_source_replay_is_bounded_and_does_not_repeat_refusals() {
    let changed_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&changed_attempts);
    let changed = super::blocking_stream_with_replay(
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(ApiError::Unreadable(Box::new(
                kronika_reader::ReaderError::Io(std::io::Error::from(
                    std::io::ErrorKind::Interrupted,
                )),
            )))
        },
        AcceptedEncodings::default(),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        changed_attempts.load(std::sync::atomic::Ordering::Relaxed),
        2
    );

    let broken_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&broken_attempts);
    let broken = super::blocking_stream_with_replay(
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(ApiError::Unreadable(Box::new(
                kronika_reader::ReaderError::Io(std::io::Error::from(
                    std::io::ErrorKind::InvalidData,
                )),
            )))
        },
        AcceptedEncodings::default(),
    )
    .await;
    assert_eq!(broken.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        broken_attempts.load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    let refused_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&refused_attempts);
    let refused = super::blocking_stream_with_replay(
        move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(ApiError::NoSuchSegment)
        },
        AcceptedEncodings::default(),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        refused_attempts.load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn mcp_access_body_carries_the_basic_value_only_when_authentication_is_on() {
    let config = |authentication_required| crate::config::Config {
        data_root: std::path::PathBuf::from("/nonexistent"),
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: account(),
        authentication_required,
        sources: crate::config::SOURCE_OS,
        synthetic_demo: false,
        export_gate: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    };

    let with_auth: serde_json::Value =
        serde_json::from_str(&crate::mcp_access_body(&config(true))).expect("json");
    assert_eq!(with_auth["record"], "mcp_access");
    assert_eq!(with_auth["authorization"], AUTHORIZATION);

    let open: serde_json::Value =
        serde_json::from_str(&crate::mcp_access_body(&config(false))).expect("json");
    assert!(open["authorization"].is_null());
}
