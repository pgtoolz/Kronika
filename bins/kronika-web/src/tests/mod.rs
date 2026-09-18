//! Cross-module web tests. Unit tests live here too and are loaded by their
//! owning modules through `#[path]` so they retain access to private items.

pub(crate) mod artifacts;
mod http;
mod multi_layout;

/// Feed a one-shot fixture through the production streaming worker.
pub(crate) async fn stream_once(
    prepare: impl FnOnce() -> Result<crate::api::Prepared, crate::api::ApiError> + Send + 'static,
    accepted: crate::encoding::AcceptedEncodings,
) -> hyper::Response<crate::body::WebBody> {
    let mut prepare = Some(prepare);
    crate::streaming::prepare_response(
        move || prepare.take().ok_or(crate::api::ApiError::BadCursor)?(),
        accepted,
    )
    .await
}
