use std::io::Read as _;
use std::sync::atomic::Ordering;

use flate2::read::GzDecoder;
use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::body::Bytes;
use hyper::header::{
    CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
    HeaderValue, VARY, X_CONTENT_TYPE_OPTIONS,
};
use sha2::{Digest as _, Sha256};

use super::{
    DecodeProbe, IdentityBody, UI_CSP, UI_GZIP, UI_GZIP_ETAG, UI_GZIP_LEN, UI_IDENTITY_CHUNK_BYTES,
    UI_IDENTITY_ETAG, UI_IDENTITY_LEN, UI_IDENTITY_SHA256, response, response_observed,
};
use crate::encoding::ContentCoding;

#[tokio::test]
async fn identity_get_is_readable_and_has_identity_headers() {
    let probe = DecodeProbe::default();
    let get = response_observed(false, None, ContentCoding::Identity, probe.clone())
        .expect("identity response");
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        get.headers().get(CONTENT_TYPE),
        Some(&HeaderValue::from_static("text/html; charset=utf-8"))
    );
    assert!(!get.headers().contains_key(CONTENT_ENCODING));
    assert_eq!(
        get.headers().get(CONTENT_LENGTH),
        Some(&HeaderValue::from_static(UI_IDENTITY_LEN))
    );
    assert_eq!(
        get.headers().get(ETAG),
        Some(&HeaderValue::from_static(UI_IDENTITY_ETAG))
    );
    assert_common_headers(get.headers());
    assert_eq!(probe.0.starts.load(Ordering::Relaxed), 0);

    let (body, frames) = collect_frames(get).await;
    assert_eq!(body.len().to_string(), UI_IDENTITY_LEN);
    assert!(body.starts_with(b"<!doctype html>"));
    assert_eq!(strong_etag(&body), UI_IDENTITY_ETAG);
    assert_eq!(UI_IDENTITY_ETAG, format!("\"{UI_IDENTITY_SHA256}\""));
    assert!(frames.len() > 1);
    assert!(
        frames
            .iter()
            .all(|length| *length <= UI_IDENTITY_CHUNK_BYTES)
    );
    assert_eq!(probe.0.starts.load(Ordering::Relaxed), 1);
    assert_eq!(probe.0.completions.load(Ordering::Relaxed), 1);
    assert_eq!(probe.0.failures.load(Ordering::Relaxed), 0);
    assert_eq!(probe.0.frames.load(Ordering::Relaxed), frames.len());
    assert_eq!(probe.0.bytes.load(Ordering::Relaxed), body.len());
    assert_eq!(
        probe.0.yields.load(Ordering::Relaxed),
        frames.len().saturating_sub(1)
    );
    assert!(probe.0.max_frame.load(Ordering::Relaxed) <= UI_IDENTITY_CHUNK_BYTES);
}

#[tokio::test]
async fn gzip_get_preserves_the_committed_representation_without_decoding() {
    let probe = DecodeProbe::default();
    let get =
        response_observed(false, None, ContentCoding::Gzip, probe.clone()).expect("gzip response");
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(
        get.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    assert_eq!(
        get.headers().get(CONTENT_LENGTH),
        Some(&HeaderValue::from_static(UI_GZIP_LEN))
    );
    assert_eq!(
        get.headers().get(ETAG),
        Some(&HeaderValue::from_static(UI_GZIP_ETAG))
    );
    assert_common_headers(get.headers());
    assert_eq!(
        get.into_body()
            .collect()
            .await
            .expect("gzip body")
            .to_bytes(),
        UI_GZIP
    );
    assert_eq!(probe.0.starts.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn head_uses_selected_representation_headers_without_a_body() {
    for (coding, length, content_encoding) in [
        (ContentCoding::Identity, UI_IDENTITY_LEN, None),
        (ContentCoding::Gzip, UI_GZIP_LEN, Some("gzip")),
    ] {
        let probe = DecodeProbe::default();
        let head = response_observed(true, None, coding, probe.clone()).expect("HEAD response");
        assert_eq!(head.status(), StatusCode::OK, "{coding:?}");
        assert_eq!(
            head.headers().get(CONTENT_LENGTH),
            Some(&HeaderValue::from_static(length)),
            "{coding:?}"
        );
        assert_eq!(
            head.headers()
                .get(CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            content_encoding,
            "{coding:?}"
        );
        assert!(
            head.into_body()
                .collect()
                .await
                .expect("HEAD body")
                .to_bytes()
                .is_empty(),
            "{coding:?}"
        );
        assert_eq!(probe.0.starts.load(Ordering::Relaxed), 0, "{coding:?}");
    }
}

#[tokio::test]
async fn validators_are_representation_specific() {
    assert_ne!(UI_IDENTITY_ETAG, UI_GZIP_ETAG);
    for (coding, current, other) in [
        (ContentCoding::Identity, UI_IDENTITY_ETAG, UI_GZIP_ETAG),
        (ContentCoding::Gzip, UI_GZIP_ETAG, UI_IDENTITY_ETAG),
    ] {
        for offered in [
            current.to_owned(),
            format!("W/{current}"),
            format!("\"old\", {current}"),
            "*".to_owned(),
        ] {
            let probe = DecodeProbe::default();
            let not_modified = response_observed(false, Some(&offered), coding, probe.clone())
                .expect("conditional response");
            assert_eq!(
                not_modified.status(),
                StatusCode::NOT_MODIFIED,
                "{coding:?} {offered}"
            );
            assert_eq!(
                not_modified.headers().get(ETAG),
                Some(&HeaderValue::from_static(current)),
                "{coding:?} {offered}"
            );
            assert!(
                not_modified
                    .into_body()
                    .collect()
                    .await
                    .expect("304 body")
                    .to_bytes()
                    .is_empty(),
                "{coding:?} {offered}"
            );
            assert_eq!(
                probe.0.starts.load(Ordering::Relaxed),
                0,
                "{coding:?} {offered}"
            );
        }
        assert_eq!(
            response(false, Some(other), coding)
                .expect("other representation validator")
                .status(),
            StatusCode::OK,
            "{coding:?}"
        );
    }
}

#[tokio::test]
async fn damaged_truncated_trailing_or_length_mismatched_stream_is_rejected() {
    let expected_len = UI_IDENTITY_LEN.parse().expect("identity length");
    let mut damaged = UI_GZIP.to_vec();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0xff;
    let mut trailing = UI_GZIP.to_vec();
    trailing.push(0);
    for (gzip, length) in [
        (damaged, expected_len),
        (UI_GZIP[..UI_GZIP.len() - 8].to_vec(), expected_len),
        (trailing, expected_len),
        (UI_GZIP.to_vec(), expected_len - 1),
    ] {
        let probe = DecodeProbe::default();
        let body = IdentityBody::new_observed(Bytes::from(gzip), length, probe.clone());
        assert!(body.collect().await.is_err());
        assert_eq!(probe.0.failures.load(Ordering::Relaxed), 1);
        assert_eq!(probe.0.completions.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn repeated_identity_responses_decode_independently() {
    let expected_len = UI_IDENTITY_LEN.parse::<usize>().expect("identity length");
    let probe = DecodeProbe::default();
    for _request in 0..3 {
        let response = response_observed(false, None, ContentCoding::Identity, probe.clone())
            .expect("identity response");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("identity body")
            .to_bytes();
        assert_eq!(strong_etag(&body), UI_IDENTITY_ETAG);
    }
    assert_eq!(probe.0.starts.load(Ordering::Relaxed), 3);
    assert_eq!(probe.0.completions.load(Ordering::Relaxed), 3);
    assert_eq!(probe.0.failures.load(Ordering::Relaxed), 0);
    assert_eq!(probe.0.bytes.load(Ordering::Relaxed), expected_len * 3);
}

#[test]
fn production_source_has_no_retained_identity_buffer() {
    let source = include_str!("../ui.rs");
    assert!(!source.contains("LazyLock"));
    assert!(!source.contains("read_to_end"));
    assert!(size_of::<IdentityBody>() < UI_IDENTITY_CHUNK_BYTES);
}

#[test]
fn committed_bytes_have_the_reproducible_gzip_header() {
    assert_eq!(UI_GZIP.get(..4), Some([0x1f, 0x8b, 8, 0].as_slice()));
    assert_eq!(UI_GZIP.get(4..8), Some([0, 0, 0, 0].as_slice()));
    assert_eq!(strong_etag(UI_GZIP), UI_GZIP_ETAG);
}

#[test]
fn content_security_policy_allows_both_inline_scripts() {
    let mut html = String::new();
    GzDecoder::new(UI_GZIP)
        .read_to_string(&mut html)
        .expect("decode embedded UI");
    let mut scripts = Vec::new();
    let mut tail = html.as_str();
    while let Some(start) = tail.find("<script>") {
        let body = tail
            .get(start..)
            .and_then(|script| script.strip_prefix("<script>"))
            .expect("script starts at a UTF-8 boundary");
        let (script, rest) = body
            .split_once("</script>")
            .expect("complete inline script");
        scripts.push(script);
        tail = rest;
    }
    assert_eq!(scripts.len(), 2);
    let script_sources = UI_CSP
        .split(';')
        .find(|directive| directive.trim_start().starts_with("script-src "))
        .expect("script-src directive")
        .split_ascii_whitespace()
        .skip(1)
        .collect::<Vec<_>>();
    assert_eq!(script_sources.len(), 2);
    assert!(
        script_sources
            .iter()
            .all(|source| source.starts_with("'sha256-") && source.ends_with('\''))
    );
}

fn assert_common_headers(headers: &hyper::HeaderMap) {
    assert_eq!(
        headers.get(CACHE_CONTROL),
        Some(&HeaderValue::from_static("private,no-cache"))
    );
    assert_eq!(
        headers.get(VARY),
        Some(&HeaderValue::from_static("Authorization, Accept-Encoding"))
    );
    assert_eq!(
        headers.get(X_CONTENT_TYPE_OPTIONS),
        Some(&HeaderValue::from_static("nosniff"))
    );
    assert_eq!(
        headers
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').any(|directive| {
                directive
                    .trim()
                    .split_ascii_whitespace()
                    .eq(["form-action", "'none'"])
            })),
        Some(true)
    );
}

fn strong_etag(bytes: &[u8]) -> String {
    format!("\"{:x}\"", Sha256::digest(bytes))
}

async fn collect_frames(response: hyper::Response<crate::WebBody>) -> (Vec<u8>, Vec<usize>) {
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    let mut frames = Vec::new();
    while let Some(frame) = body.frame().await {
        let data = frame
            .expect("identity frame")
            .into_data()
            .expect("identity data frame");
        frames.push(data.len());
        bytes.extend_from_slice(&data);
    }
    (bytes, frames)
}
