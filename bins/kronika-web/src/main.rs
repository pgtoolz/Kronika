//! Serves authenticated Kronika HTTP resources and MCP tools over HTTP/1.1.
//!
//! Streamed API reads and response generation, plus MCP tool dispatch, run on
//! Tokio's blocking pool. The process retains no segment, decoded-data, or
//! response cache between requests.

mod api;
mod auth;
mod body;
mod budget;
mod config;
mod encoding;
mod export;
mod help;
mod mcp;
mod query_adapter;
mod route;
mod ui;

use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, COOKIE, ETAG, HeaderValue,
    IF_NONE_MATCH, ORIGIN, SET_COOKIE, VARY, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use api::{ApiError, CachePolicy};
use body::{BodyError, BodyItem, BodyProducer, ChannelBody, StreamHead};
use config::Config;
use encoding::{AcceptedEncodings, ContentCoding};
use route::RouteError;

type WebBody = UnsyncBoxBody<Bytes, BodyError>;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    if let Some(argument) = arguments.next() {
        anyhow::ensure!(
            arguments.next().is_none(),
            "unexpected arguments; use kronika-web --help"
        );
        if argument == "--help" || argument == "-h" {
            print!("{}", help::HELP);
            return Ok(());
        }
        if argument == "--version" {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        anyhow::bail!(
            "unexpected argument {}; use kronika-web --help",
            argument.display()
        );
    }
    run_web()
}

#[tokio::main]
async fn run_web() -> Result<()> {
    let config = Arc::new(Config::from_env()?);
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("listen on {}", config.listen))?;
    println!("ready {}", config.listen);

    loop {
        let (stream, _peer) = listener.accept().await.context("accept a connection")?;
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let service = service_fn(move |request| answer(Arc::clone(&config), request));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("kronika-web: connection ended: {error}");
            }
        });
    }
}

async fn answer(
    config: Arc<Config>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<WebBody>, Infallible> {
    let routed = if config.authentication_required {
        route_request(&config.account, &request)
    } else {
        route_request_without_authentication(&request)
    };
    let target = match routed {
        Ok(target) => target,
        Err(error) => return Ok(error.response()),
    };
    let if_none_match = if_none_match_values(request.headers());
    Ok(match target {
        RequestTarget::Ui { head, coding } => ui::response(head, if_none_match.as_deref(), coding)
            .unwrap_or_else(|error| {
                eprintln!("kronika-web: serve embedded interface: {error}");
                failed()
            }),
        RequestTarget::Session(session) => {
            session_response(&config.account, session).unwrap_or_else(failed)
        }
        RequestTarget::Api { route, accepted } => {
            if let route::Route::Export(range) = route {
                export::response(
                    config.data_root.clone(),
                    range,
                    Arc::clone(&config.export_gate),
                )
                .await
            } else if matches!(route, route::Route::McpAccess) {
                mcp::with_private_headers(json_response(StatusCode::OK, mcp_access_body(&config)))
            } else if matches!(route, route::Route::InstanceLabel) {
                match tokio::task::spawn_blocking(move || largest_database(&config)).await {
                    Ok(database) => {
                        let cache_for_a_day = database.is_some();
                        let response =
                            json_response(StatusCode::OK, instance_label_body(database.as_deref()));
                        if cache_for_a_day {
                            day_cached_private(response)
                        } else {
                            response
                        }
                    }
                    Err(error) => {
                        eprintln!("kronika-web: instance label task: {error}");
                        failed()
                    }
                }
            } else {
                streamed(config, route, if_none_match, accepted).await
            }
        }
        RequestTarget::Mcp => mcp::response(config, request).await,
    })
}

fn route_request<B>(
    account: &config::Account,
    request: &Request<B>,
) -> Result<RequestTarget, RequestError> {
    route_request_at(account, request, unix_time())
}

fn route_request_without_authentication<B>(
    request: &Request<B>,
) -> Result<RequestTarget, RequestError> {
    route_request_with_authentication(None, request, unix_time())
}

fn route_request_at<B>(
    account: &config::Account,
    request: &Request<B>,
    now: u64,
) -> Result<RequestTarget, RequestError> {
    route_request_with_authentication(Some(account), request, now)
}

fn route_request_with_authentication<B>(
    account: Option<&config::Account>,
    request: &Request<B>,
    now: u64,
) -> Result<RequestTarget, RequestError> {
    let path = request.uri().path();
    if ui::is_path(path) {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return Err(RequestError::MethodNotAllowed("GET, HEAD"));
        }
        let accepted = AcceptedEncodings::from_headers(request.headers())
            .ok_or(RequestError::UiEncodingNotAcceptable)?;
        return Ok(RequestTarget::Ui {
            head: request.method() == Method::HEAD,
            coding: accepted.for_ui(),
        });
    }
    if path == "/auth/session" && request.uri().query().is_none() {
        if request.method() == Method::GET {
            return Ok(RequestTarget::Session(SessionTarget::Check {
                admitted: account
                    .is_none_or(|account| admitted_session(account, request.headers(), now)),
            }));
        }
        if request.method() == Method::POST {
            let Some(account) = account else {
                return Ok(RequestTarget::Session(SessionTarget::Check {
                    admitted: true,
                }));
            };
            let admitted = matches!(
                authorization(request.headers()),
                SingleHeader::Value(value) if auth::admits_basic(account, Some(value))
            );
            let secure = if admitted {
                secure_session_cookie(request.headers())?
            } else {
                false
            };
            return Ok(RequestTarget::Session(SessionTarget::Login {
                issued_at: admitted.then_some(now),
                secure,
            }));
        }
        if request.method() == Method::DELETE {
            return Ok(RequestTarget::Session(SessionTarget::Clear {
                secure: secure_session_cookie(request.headers())?,
            }));
        }
        return Ok(RequestTarget::Session(SessionTarget::MethodNotAllowed));
    }
    if path == "/mcp" && request.uri().query().is_none() {
        reject_browser_origin(request.headers())?;
        if account.is_some_and(|account| !admitted_api(account, request.headers(), now)) {
            return Err(RequestError::Unauthorized {
                challenge: !is_ui_request(request.headers()),
            });
        }
        return Ok(RequestTarget::Mcp);
    }
    if path != "/api" && !path.starts_with("/api/") {
        return Err(RequestError::Route(RouteError::NoSuchPath));
    }
    if account.is_some_and(|account| !admitted_api(account, request.headers(), now)) {
        return Err(RequestError::Unauthorized {
            challenge: !is_ui_request(request.headers()),
        });
    }
    let route = route::parse(path, request.uri().query()).map_err(RequestError::Route)?;
    if request.method() != Method::GET {
        return Err(RequestError::MethodNotAllowed("GET"));
    }
    let accepted = AcceptedEncodings::from_headers(request.headers())
        .ok_or(RequestError::ApiEncodingNotAcceptable)?;
    if matches!(&route, route::Route::Export(_)) && !accepted.accepts_identity() {
        return Err(RequestError::ApiEncodingNotAcceptable);
    }
    Ok(RequestTarget::Api { route, accepted })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestTarget {
    Ui {
        head: bool,
        coding: ContentCoding,
    },
    Session(SessionTarget),
    Api {
        route: route::Route,
        accepted: AcceptedEncodings,
    },
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionTarget {
    Check {
        admitted: bool,
    },
    Login {
        issued_at: Option<u64>,
        secure: bool,
    },
    Clear {
        secure: bool,
    },
    MethodNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestError {
    Unauthorized { challenge: bool },
    Route(RouteError),
    MethodNotAllowed(&'static str),
    UiEncodingNotAcceptable,
    ApiEncodingNotAcceptable,
    InvalidOrigin,
    OriginNotAllowed,
}

impl RequestError {
    fn response(self) -> Response<WebBody> {
        match self {
            Self::Unauthorized { challenge } => unauthorized(challenge),
            Self::Route(RouteError::NoSuchPath) => {
                refused(StatusCode::NOT_FOUND, "no_such_path", None)
            }
            Self::Route(RouteError::BadParameter(parameter)) => {
                refused(StatusCode::BAD_REQUEST, "bad_parameter", Some(&parameter))
            }
            Self::MethodNotAllowed(allow) => method_not_allowed(allow),
            Self::UiEncodingNotAcceptable => encoding_not_acceptable(false),
            Self::ApiEncodingNotAcceptable => encoding_not_acceptable(true),
            Self::InvalidOrigin => refused(StatusCode::BAD_REQUEST, "invalid_origin", None),
            Self::OriginNotAllowed => refused(StatusCode::FORBIDDEN, "origin_not_allowed", None),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SingleHeader<'a> {
    Absent,
    Value(&'a str),
    Invalid,
}

impl fmt::Debug for SingleHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("Absent"),
            Self::Value(_) => f.write_str("Value([redacted])"),
            Self::Invalid => f.write_str("Invalid"),
        }
    }
}

fn authorization(headers: &HeaderMap) -> SingleHeader<'_> {
    unique_header(headers.get_all(AUTHORIZATION).iter())
}

fn cookie_header(headers: &HeaderMap) -> SingleHeader<'_> {
    unique_header(headers.get_all(COOKIE).iter())
}

fn unique_header<'a>(mut values: impl Iterator<Item = &'a HeaderValue>) -> SingleHeader<'a> {
    let Some(value) = values.next() else {
        return SingleHeader::Absent;
    };
    if values.next().is_some() {
        return SingleHeader::Invalid;
    }
    value
        .to_str()
        .map_or(SingleHeader::Invalid, SingleHeader::Value)
}

fn admitted_session(account: &config::Account, headers: &HeaderMap, now: u64) -> bool {
    matches!(
        cookie_header(headers),
        SingleHeader::Value(value) if auth::admits_session(account, Some(value), now)
    )
}

fn admitted_api(account: &config::Account, headers: &HeaderMap, now: u64) -> bool {
    match authorization(headers) {
        SingleHeader::Value(value) => auth::admits_basic(account, Some(value)),
        SingleHeader::Absent => admitted_session(account, headers, now),
        SingleHeader::Invalid => false,
    }
}

/// Rejects every `/mcp` request with an `Origin` header. A missing header continues
/// to authentication; malformed or duplicate headers are bad requests, and valid
/// browser origins are forbidden.
fn reject_browser_origin(headers: &HeaderMap) -> Result<(), RequestError> {
    match unique_header(headers.get_all(ORIGIN).iter()) {
        SingleHeader::Absent => Ok(()),
        SingleHeader::Invalid => Err(RequestError::InvalidOrigin),
        SingleHeader::Value(text) => {
            if origin_secure(text).is_some() {
                Err(RequestError::OriginNotAllowed)
            } else {
                Err(RequestError::InvalidOrigin)
            }
        }
    }
}

fn secure_session_cookie(headers: &HeaderMap) -> Result<bool, RequestError> {
    match unique_header(headers.get_all(ORIGIN).iter()) {
        SingleHeader::Value(text) => origin_secure(text).ok_or(RequestError::InvalidOrigin),
        SingleHeader::Absent | SingleHeader::Invalid => Err(RequestError::InvalidOrigin),
    }
}

fn origin_secure(text: &str) -> Option<bool> {
    let uri: hyper::Uri = text.parse().ok()?;
    if uri.authority().is_none() || uri.path() != "/" || uri.query().is_some() {
        return None;
    }
    match uri.scheme_str() {
        Some("http") => Some(false),
        Some("https") => Some(true),
        _ => None,
    }
}

fn is_ui_request(headers: &HeaderMap) -> bool {
    matches!(
        unique_header(headers.get_all("x-kronika-ui").iter()),
        SingleHeader::Value("1")
    )
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn if_none_match_values(headers: &HeaderMap) -> Option<String> {
    let mut combined = String::new();
    for value in headers.get_all(IF_NONE_MATCH) {
        let value = value.to_str().ok()?;
        if !combined.is_empty() {
            combined.push(',');
        }
        combined.push_str(value);
    }
    (!combined.is_empty()).then_some(combined)
}

async fn streamed(
    config: Arc<Config>,
    route: route::Route,
    if_none_match: Option<String>,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    blocking_stream_with_replay(
        move || {
            api::prepare_with_demo(
                &config.data_root,
                config.sources,
                config.synthetic_demo,
                route.clone(),
                if_none_match.as_deref(),
            )
        },
        accepted,
    )
    .await
}

#[cfg(test)]
async fn blocking_stream(
    prepare: impl FnOnce() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let mut prepare = Some(prepare);
    blocking_stream_inner(
        move || {
            let Some(prepare) = prepare.take() else {
                return Err(ApiError::BadCursor);
            };
            prepare()
        },
        accepted,
        false,
    )
    .await
}

async fn blocking_stream_with_replay(
    prepare: impl FnMut() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    blocking_stream_inner(prepare, accepted, true).await
}

async fn blocking_stream_inner(
    mut prepare: impl FnMut() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
    replay_source_change: bool,
) -> Response<WebBody> {
    let (body_tx, body_rx) = mpsc::channel::<BodyItem>(8);
    let (head_tx, head_rx) = oneshot::channel::<Result<StreamHead, ApiError>>();
    let handle = tokio::task::spawn_blocking(move || {
        let mut head_tx = Some(head_tx);
        let mut producer: Option<BodyProducer> = None;
        let mut replayed = false;
        loop {
            let prepared = match prepare() {
                Ok(prepared) => prepared,
                Err(error)
                    if replay_source_change && !replayed && error.source_changed_during_read() =>
                {
                    replayed = true;
                    continue;
                }
                Err(error) => {
                    if let Some(producer) = producer.take() {
                        producer.fail(error);
                    } else if let Some(head_tx) = head_tx.take() {
                        let _sent = head_tx.send(Err(error));
                    }
                    return;
                }
            };
            let meta = prepared.meta();
            if meta.status == StatusCode::NOT_MODIFIED {
                if let Some(producer) = producer.take() {
                    producer.complete_not_modified(meta);
                } else if let Some(head_tx) = head_tx.take() {
                    let _sent = head_tx.send(Ok(StreamHead::not_modified(meta)));
                }
                return;
            }
            if producer.is_some() {
                let restarted = producer
                    .as_mut()
                    .is_some_and(|producer| producer.restart(meta));
                if !restarted {
                    if let Some(producer) = producer.take() {
                        producer.fail(ApiError::Unreadable(Box::new(std::io::Error::other(
                            "response replay began after headers",
                        ))));
                    }
                    return;
                }
            } else {
                let Some(head_tx) = head_tx.take() else {
                    return;
                };
                producer = Some(BodyProducer::new(accepted, meta, head_tx, body_tx.clone()));
            }
            let cancellation_tx = body_tx.clone();
            let cancelled = || cancellation_tx.is_closed();
            let Some(current) = producer.as_mut() else {
                return;
            };
            let result = prepared.stream(&mut |bytes| current.emit(&bytes), &cancelled);
            match result {
                Ok(()) => {
                    if let Some(producer) = producer.take() {
                        producer.complete();
                    }
                    return;
                }
                Err(error)
                    if replay_source_change
                        && !replayed
                        && error.source_changed_during_read()
                        && current.can_restart() =>
                {
                    replayed = true;
                }
                Err(error) => {
                    if let Some(producer) = producer.take() {
                        producer.fail(error);
                    }
                    return;
                }
            }
        }
    });
    drop(handle);

    match head_rx.await {
        Ok(Ok(head)) => response_from_meta(head, body_rx),
        Ok(Err(error)) => {
            if matches!(error, ApiError::Unreadable(_)) {
                eprintln!("kronika-web: resource preparation failed: {error}");
            }
            refused(
                api::api_error_status(&error),
                error.code(),
                error.parameter(),
            )
        }
        Err(_closed) => failed(),
    }
}

fn response_from_meta(head: StreamHead, receiver: mpsc::Receiver<BodyItem>) -> Response<WebBody> {
    let body = if head.meta.status == StatusCode::NOT_MODIFIED {
        Full::new(Bytes::new())
            .map_err(BodyError::from)
            .boxed_unsync()
    } else {
        ChannelBody { receiver }.boxed_unsync()
    };
    let mut response = Response::new(body);
    *response.status_mut() = head.meta.status;
    common_headers(&mut response, head.meta.cache);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    if head.meta.status != StatusCode::NOT_MODIFIED {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
        );
    }
    if let Some(coding) = head.coding.and_then(ContentCoding::header) {
        response
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(coding));
    }
    if let Some(etag) = head.meta.etag
        && let Ok(value) = HeaderValue::from_str(&etag)
    {
        response.headers_mut().insert(ETAG, value);
    }
    response
}

fn refused(status: StatusCode, error: &str, parameter: Option<&str>) -> Response<WebBody> {
    let value = parameter.map_or_else(
        || json!({ "error": error }),
        |parameter| json!({ "error": error, "parameter": parameter }),
    );
    json_response(status, value.to_string())
}

fn failed() -> Response<WebBody> {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "unreadable" }).to_string(),
    )
}

fn unauthorized(challenge: bool) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "unauthorized" }).to_string(),
    );
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, X-Kronika-UI"),
    );
    if challenge {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"kronika\""),
        );
    }
    response
}

fn session_response(account: &config::Account, target: SessionTarget) -> Option<Response<WebBody>> {
    let (status, cookie, allow) = match target {
        SessionTarget::Check { admitted: true } => (StatusCode::NO_CONTENT, None, None),
        SessionTarget::Check { admitted: false }
        | SessionTarget::Login {
            issued_at: None, ..
        } => (StatusCode::UNAUTHORIZED, None, None),
        SessionTarget::Login {
            issued_at: Some(now),
            secure,
        } => (
            StatusCode::NO_CONTENT,
            Some(auth::issue_cookie(account, now, secure)),
            None,
        ),
        SessionTarget::Clear { secure } => (
            StatusCode::NO_CONTENT,
            Some(auth::clear_cookie(secure)),
            None,
        ),
        SessionTarget::MethodNotAllowed => (
            StatusCode::METHOD_NOT_ALLOWED,
            None,
            Some("GET, POST, DELETE"),
        ),
    };
    let mut response = Response::new(
        Full::new(Bytes::new())
            .map_err(BodyError::from)
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, X-Kronika-UI"),
    );
    if let Some(cookie) = cookie {
        let value = HeaderValue::from_str(&cookie).ok()?;
        response.headers_mut().insert(SET_COOKIE, value);
    }
    if let Some(allow) = allow {
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static(allow));
    }
    Some(response)
}

fn method_not_allowed(allow: &'static str) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({ "error": "method_not_allowed" }).to_string(),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

fn encoding_not_acceptable(api: bool) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::NOT_ACCEPTABLE,
        json!({ "error": "encoding_not_acceptable" }).to_string(),
    );
    if api {
        response.headers_mut().insert(
            VARY,
            HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
        );
    } else {
        ui::set_vary(&mut response);
    }
    response
}

/// The same `Authorization` value `/mcp` accepts, or null when the server
/// runs without authentication.
fn mcp_access_body(config: &Config) -> String {
    use base64::Engine as _;
    let authorization = config.authentication_required.then(|| {
        let credentials = format!("{}:{}", config.account.user, config.account.password);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        )
    });
    serde_json::json!({ "record": "mcp_access", "authorization": authorization }).to_string()
}

/// Marks a response reusable by the operator's browser for a day.
fn day_cached_private(mut response: Response<WebBody>) -> Response<WebBody> {
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private,max-age=86400"),
    );
    response
}

/// `{"record":"instance_label","database":…}`, null without a label.
fn instance_label_body(database: Option<&str>) -> String {
    serde_json::json!({
        "record": "instance_label",
        "database": database,
    })
    .to_string()
}

/// Operators call an instance by its biggest database, so the label is
/// the largest database in the newest recorded relation snapshot.
fn largest_database(config: &Config) -> Option<String> {
    match try_largest_database(config) {
        Ok(database) => database,
        Err(error) => {
            eprintln!("kronika-web: instance label: {error}");
            None
        }
    }
}

fn try_largest_database(config: &Config) -> Result<Option<String>, ApiError> {
    let dataset = Arc::new(query_adapter::NativeDataset::from_root(&config.data_root)?);
    let context = kronika_query::QueryContext::new(dataset, config.sources, config.synthetic_demo);
    let query = kronika_query::snapshot::CurrentSnapshotQuery {
        logical_name: "pg_stat_user_tables".to_owned(),
        fields: vec!["displayed_storage_bytes".to_owned()],
        order: Some(kronika_query::snapshot::FinderOrder {
            field: "displayed_storage_bytes".to_owned(),
            direction: kronika_query::Order::Desc,
        }),
        group: Some(route::RelationGroup::Database),
        limit: 1,
    };
    let Some(result) =
        kronika_query::snapshot::execute_current_relation(&context, query, &|| false)?
    else {
        return Ok(None);
    };
    Ok(result
        .rows
        .into_iter()
        .next()
        .and_then(|row| row.key.text("datname").map(str::to_owned)))
}

fn json_response(status: StatusCode, body: String) -> Response<WebBody> {
    let mut response = Response::new(
        Full::new(Bytes::from(body))
            .map_err(BodyError::from)
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    common_headers(&mut response, CachePolicy::NoStore);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn common_headers(response: &mut Response<WebBody>, cache: CachePolicy) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache.header()));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
}

#[cfg(test)]
mod tests;
