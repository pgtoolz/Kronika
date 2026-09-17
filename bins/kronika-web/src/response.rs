//! HTTP error responses and cache headers shared by the API and export handlers.

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderValue, VARY, WWW_AUTHENTICATE};
use hyper::{Response, StatusCode};
use serde_json::json;

use crate::api::CachePolicy;
use crate::body::{BodyError, WebBody};
use crate::ui;

pub(crate) fn json_response(status: StatusCode, body: String) -> Response<WebBody> {
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

pub(crate) fn common_headers(response: &mut Response<WebBody>, cache: CachePolicy) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache.header()));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
}

pub(crate) fn refused(
    status: StatusCode,
    error: &str,
    parameter: Option<&str>,
) -> Response<WebBody> {
    let value = parameter.map_or_else(
        || json!({ "error": error }),
        |parameter| json!({ "error": error, "parameter": parameter }),
    );
    json_response(status, value.to_string())
}

pub(crate) fn failed() -> Response<WebBody> {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "unreadable" }).to_string(),
    )
}

pub(crate) fn unauthorized(challenge: bool) -> Response<WebBody> {
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

pub(crate) fn method_not_allowed(allow: &'static str) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({ "error": "method_not_allowed" }).to_string(),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

pub(crate) fn encoding_not_acceptable(api: bool) -> Response<WebBody> {
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
