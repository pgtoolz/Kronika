//! Prepare API responses on blocking workers and deliver bounded response bodies.

use std::sync::Arc;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_ENCODING, CONTENT_TYPE, ETAG, HeaderValue, VARY};
use hyper::{Response, StatusCode};
use tokio::sync::{mpsc, oneshot};

use crate::api::{self, ApiError};
use crate::body::{
    BODY_CHANNEL_CAPACITY, BodyError, BodyItem, BodyProducer, ChannelBody, StreamHead, WebBody,
};
use crate::config::Config;
use crate::encoding::{AcceptedEncodings, ContentCoding};
use crate::response::{common_headers, failed, refused};
use crate::route::Route;

pub(crate) async fn response(
    config: Arc<Config>,
    route: Route,
    if_none_match: Option<String>,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    prepare_response(
        move || {
            api::prepare(
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

/// Retry one source change while the response is still staged before headers.
pub(crate) async fn prepare_response(
    mut prepare: impl FnMut() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let (body_tx, body_rx) = mpsc::channel::<BodyItem>(BODY_CHANNEL_CAPACITY);
    let (head_tx, head_rx) = oneshot::channel::<Result<StreamHead, ApiError>>();
    let handle = tokio::task::spawn_blocking(move || {
        let mut head_tx = head_tx;
        let mut replayed = false;
        loop {
            let prepared = match prepare() {
                Ok(prepared) => prepared,
                Err(error) if !replayed && error.source_changed_during_read() => {
                    replayed = true;
                    continue;
                }
                Err(error) => {
                    let _sent = head_tx.send(Err(error));
                    return;
                }
            };
            let meta = prepared.meta();
            if meta.status == StatusCode::NOT_MODIFIED {
                let _sent = head_tx.send(Ok(StreamHead::not_modified(meta)));
                return;
            }
            let mut producer = BodyProducer::new(accepted, meta, head_tx, body_tx.clone());
            let result =
                prepared.stream(&mut |bytes| producer.emit(&bytes), &|| body_tx.is_closed());
            match result {
                Ok(()) => {
                    producer.complete();
                    return;
                }
                Err(error) => {
                    if !replayed
                        && error.source_changed_during_read()
                        && let Some(pending_head) = producer.take_staged_head()
                    {
                        // Discard this attempt's prefix before preparing a fresh generation.
                        head_tx = pending_head;
                        replayed = true;
                    } else {
                        producer.fail(error);
                        return;
                    }
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

pub(crate) fn response_from_meta(
    head: StreamHead,
    receiver: mpsc::Receiver<BodyItem>,
) -> Response<WebBody> {
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
