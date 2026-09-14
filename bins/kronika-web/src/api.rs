//! Preparing blocking resource reads and streaming small self-describing records.

use std::path::Path;

use hyper::StatusCode;
use kronika_query::{
    QueryContext, QueryError, QueryIdentity, QueryRequest, QuerySink, QueryStability,
};
use sha2::{Digest as _, Sha256};

use crate::encoding::etag_matches;
use crate::route::Route;

#[cfg(test)]
mod tests;

/// Cache policy applied centrally after preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachePolicy {
    /// Mutable catalog, active data, and errors.
    NoStore,
    Revalidate,
    /// Immutable finished history or rows.
    Immutable,
}

impl CachePolicy {
    pub(crate) const fn header(self) -> &'static str {
        match self {
            Self::NoStore => "private,no-store",
            Self::Revalidate => "private,no-cache",
            Self::Immutable => "private,max-age=31536000,immutable",
        }
    }
}

/// Headers known before a streamed body starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseMeta {
    pub(crate) status: StatusCode,
    pub(crate) cache: CachePolicy,
    pub(crate) etag: Option<String>,
}

impl ResponseMeta {
    const fn ok_with_etag(cache: CachePolicy, etag: Option<String>) -> Self {
        Self {
            status: StatusCode::OK,
            cache,
            etag,
        }
    }
}

/// A prepared response whose disk/Parquet work remains on the blocking thread.
pub(crate) enum Prepared {
    Query(Box<PreparedQuery>),
    Empty(ResponseMeta),
}

pub(crate) struct PreparedQuery {
    execution: kronika_query::QueryExecution,
    meta: ResponseMeta,
}

impl Prepared {
    /// Response status and caching, available before the first body record.
    pub(crate) fn meta(&self) -> ResponseMeta {
        match self {
            Self::Query(prepared) => prepared.meta.clone(),
            Self::Empty(meta) => meta.clone(),
        }
    }

    /// Emit newline-delimited JSON records until complete or the client leaves.
    pub(crate) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        match self {
            Self::Query(prepared) => {
                let mut sink = NativeSink { emit, cancelled };
                prepared.execution.stream(&mut sink)
            }
            Self::Empty(_meta) => Ok(()),
        }
    }
}

pub(crate) type ApiError = QueryError;

pub(crate) const fn api_error_status(error: &ApiError) -> StatusCode {
    match error {
        ApiError::NoSuchSegment | ApiError::NoSuchSection => StatusCode::NOT_FOUND,
        ApiError::NoSuchColumn(_)
        | ApiError::MixedUnits(_)
        | ApiError::BadFilter(_)
        | ApiError::BadCursor
        | ApiError::BadLocator(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

struct NativeSink<'a, E, C> {
    emit: &'a mut E,
    cancelled: &'a C,
}

impl<E, C> QuerySink for NativeSink<'_, E, C>
where
    E: FnMut(Vec<u8>) -> bool,
    C: Fn() -> bool,
{
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        (self.emit)(bytes)
    }

    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

fn query_meta(metadata: kronika_query::QueryMetadata<'_>) -> ResponseMeta {
    let cache = match metadata.stability() {
        QueryStability::Mutable => CachePolicy::NoStore,
        QueryStability::Revalidate => CachePolicy::Revalidate,
        QueryStability::Immutable => CachePolicy::Immutable,
    };
    let etag = metadata.identity().and_then(|identity| match identity {
        QueryIdentity::IndexChecksum(checksum) => Some(format!("W/\"{checksum:08x}\"")),
        QueryIdentity::SegmentSet {
            resource,
            shape,
            segments,
        } => weak_dataset_etag(resource, shape, segments),
    });
    ResponseMeta::ok_with_etag(cache, etag)
}

fn prepared_query(execution: kronika_query::QueryExecution) -> Prepared {
    let meta = query_meta(execution.metadata());
    Prepared::Query(Box::new(PreparedQuery { execution, meta }))
}

/// Perform request validation and initial I/O outside the Tokio worker.
#[cfg(test)]
pub(crate) fn prepare(
    root: &Path,
    sources: u32,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    prepare_with_demo(root, sources, false, route, if_none_match)
}

/// Prepare a response with the deployment identity exposed in its catalog.
pub(crate) fn prepare_with_demo(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let Route::Recorded(route) = route else {
        // Answered directly in `main.rs`.
        return Err(ApiError::NoSuchSection);
    };
    let request = route.into_query()?;
    let prepared = match request {
        request @ (QueryRequest::Catalog(_)
        | QueryRequest::History(_)
        | QueryRequest::Events(_)
        | QueryRequest::RowDetail(_)
        | QueryRequest::Rows(_)
        | QueryRequest::Heatmap(_)) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            kronika_query::execute(&context, request).map(prepared_query)
        }
        request @ (QueryRequest::Index(_) | QueryRequest::Hour(_)) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(
                std::sync::Arc::<crate::query_adapter::NativeDataset>::clone(&dataset),
                sources,
                synthetic_demo,
            )
            .with_index_provider(dataset);
            kronika_query::execute(&context, request).map(prepared_query)
        }
        QueryRequest::Snapshot(request) => {
            let dataset =
                std::sync::Arc::new(crate::query_adapter::NativeDataset::from_root(root)?);
            let context = QueryContext::new(dataset, sources, synthetic_demo);
            let preparation = kronika_query::snapshot::prepare_snapshot(&context, request)?;
            let meta = query_meta(preparation.metadata());
            let concrete_validator = if_none_match.filter(|offered| offered.trim() != "*");
            if let Some(not_modified) = conditional_not_modified(meta, concrete_validator) {
                return Ok(not_modified);
            }
            preparation.finish().map(prepared_query)
        }
        _ => return Err(ApiError::NoSuchSection),
    }?;
    let meta = prepared.meta();
    if let Some(not_modified) = conditional_not_modified(meta, if_none_match) {
        return Ok(not_modified);
    }
    Ok(prepared)
}

fn conditional_not_modified(meta: ResponseMeta, if_none_match: Option<&str>) -> Option<Prepared> {
    meta.etag
        .as_deref()
        .zip(if_none_match)
        .is_some_and(|(current, offered)| etag_matches(offered, current))
        .then(|| {
            Prepared::Empty(ResponseMeta {
                status: StatusCode::NOT_MODIFIED,
                ..meta
            })
        })
}

fn weak_dataset_etag(
    resource: &str,
    shape: &str,
    segments: &[kronika_query::DatasetSegment],
) -> Option<String> {
    let mut digest = Sha256::new();
    digest.update(resource.len().to_le_bytes());
    digest.update(resource.as_bytes());
    digest.update(shape.len().to_le_bytes());
    digest.update(shape.as_bytes());
    let mut found = false;
    for segment in segments {
        if segment.kind() == kronika_reader::SegmentKind::Active {
            return None;
        }
        found = true;
        digest.update(segment.id().to_le_bytes());
        digest.update(segment.min_ts().to_le_bytes());
        digest.update(segment.max_ts().to_le_bytes());
        digest.update(segment.sections().len().to_le_bytes());
        for section in segment.sections() {
            digest.update(section.type_id.to_le_bytes());
            digest.update(section.rows.to_le_bytes());
            digest.update(section.bytes.to_le_bytes());
        }
    }
    found.then(|| format!("W/\"{:x}\"", digest.finalize()))
}
