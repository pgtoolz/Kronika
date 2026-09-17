//! Route HTTP requests, enforce authentication, and issue browser session responses.

use std::fmt;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, COOKIE, HeaderValue, IF_NONE_MATCH, ORIGIN, SET_COOKIE,
    VARY,
};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};

use crate::body::{BodyError, WebBody};
use crate::config::Account;
use crate::encoding::{AcceptedEncodings, ContentCoding};
use crate::response::{encoding_not_acceptable, method_not_allowed, refused, unauthorized};
use crate::route::RouteError;
use crate::{auth, route, ui};

pub(crate) fn route_request<B>(
    account: Option<&Account>,
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
        return SessionTarget::from_request(account, request, now).map(RequestTarget::Session);
    }
    let mcp = path == "/mcp" && request.uri().query().is_none();
    if mcp {
        reject_browser_origin(request.headers())?;
    } else if path != "/api" && !path.starts_with("/api/") {
        return Err(RequestError::Route(RouteError::NoSuchPath));
    }
    if account.is_some_and(|account| !admitted_api(account, request.headers(), now)) {
        return Err(RequestError::Unauthorized {
            challenge: !is_ui_request(request.headers()),
        });
    }
    if mcp {
        return Ok(RequestTarget::Mcp);
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
pub(crate) enum RequestTarget {
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
pub(crate) enum SessionTarget {
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

impl SessionTarget {
    fn from_request<B>(
        account: Option<&Account>,
        request: &Request<B>,
        now: u64,
    ) -> Result<Self, RequestError> {
        match *request.method() {
            Method::GET => Ok(Self::Check {
                admitted: account
                    .is_none_or(|account| admitted_session(account, request.headers(), now)),
            }),
            Method::POST => {
                let Some(account) = account else {
                    return Ok(Self::Check { admitted: true });
                };
                let admitted = matches!(
                    authorization(request.headers()),
                    SingleHeader::Value(value) if auth::admits_basic(account, Some(value))
                );
                Ok(Self::Login {
                    issued_at: admitted.then_some(now),
                    secure: admitted && secure_session_cookie(request.headers())?,
                })
            }
            Method::DELETE => Ok(Self::Clear {
                secure: secure_session_cookie(request.headers())?,
            }),
            _ => Ok(Self::MethodNotAllowed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestError {
    Unauthorized { challenge: bool },
    Route(RouteError),
    MethodNotAllowed(&'static str),
    UiEncodingNotAcceptable,
    ApiEncodingNotAcceptable,
    InvalidOrigin,
    OriginNotAllowed,
}

impl RequestError {
    pub(crate) fn response(self) -> Response<WebBody> {
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
pub(crate) enum SingleHeader<'a> {
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

pub(crate) fn authorization(headers: &HeaderMap) -> SingleHeader<'_> {
    unique_header(headers.get_all(AUTHORIZATION).iter())
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

fn admitted_session(account: &Account, headers: &HeaderMap, now: u64) -> bool {
    matches!(
        unique_header(headers.get_all(COOKIE).iter()),
        SingleHeader::Value(value) if auth::admits_session(account, Some(value), now)
    )
}

fn admitted_api(account: &Account, headers: &HeaderMap, now: u64) -> bool {
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

pub(crate) fn if_none_match_values(headers: &HeaderMap) -> Option<String> {
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

pub(crate) fn session_response(
    account: Option<&Account>,
    target: SessionTarget,
) -> Option<Response<WebBody>> {
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
            Some(auth::issue_cookie(account?, now, secure)),
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
