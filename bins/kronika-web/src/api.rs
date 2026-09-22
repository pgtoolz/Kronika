//! Prepare recorded queries and translate their metadata into HTTP cache headers.

use std::path::Path;
use std::sync::Arc;

use hyper::StatusCode;
use kronika_query::{QueryError, QueryIdentity, QueryRequest, QuerySink, QueryStability};
use sha2::{Digest as _, Sha256};

use crate::encoding::etag_matches;
use crate::query_adapter::NativeDataset;
use crate::route::Route;

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

/// A prepared response whose disk/Parquet work remains on the blocking thread.
pub(crate) enum Prepared {
    Query(Box<PreparedQuery>),
    Empty(ResponseMeta),
}

pub(crate) struct PreparedQuery {
    execution: kronika_query::QueryExecution,
    meta: ResponseMeta,
}

/// Validate the query and prepare its headers on the blocking worker.
pub(crate) fn prepare(
    root: &Path,
    sources: u32,
    synthetic_demo: bool,
    route: Route,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let Route::Recorded(route) = route else {
        // Native endpoints are handled by `server` before reaching the query engine.
        return Err(ApiError::NoSuchSection);
    };
    let request = route.into_query()?;
    let dataset = Arc::new(NativeDataset::from_root(root)?);
    let mut context = query_context(dataset, sources, synthetic_demo);
    let build = env!("KRONIKA_BUILD_COMMIT");
    if !build.is_empty() {
        context = context.with_build(build);
    }
    let execution = match request {
        QueryRequest::Snapshot(request) => {
            let preparation = kronika_query::snapshot::prepare_snapshot(&context, request)?;
            let meta = query_meta(preparation.metadata());
            // A concrete ETag can skip row decoding. `*` still requires checking
            // that the requested snapshot can be built.
            let concrete_validator = if_none_match.filter(|offered| offered.trim() != "*");
            if let Some(not_modified) = conditional_not_modified(meta, concrete_validator) {
                return Ok(not_modified);
            }
            preparation.finish()?
        }
        request => kronika_query::execute(&context, request)?,
    };
    let meta = query_meta(execution.metadata());
    if let Some(not_modified) = conditional_not_modified(meta.clone(), if_none_match) {
        return Ok(not_modified);
    }
    Ok(Prepared::Query(Box::new(PreparedQuery { execution, meta })))
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

pub(crate) fn api_error_status(error: &ApiError) -> StatusCode {
    StatusCode::from_u16(kronika_api::query_error_status(error))
        .expect("the shared API returns a valid HTTP status")
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
    ResponseMeta {
        status: StatusCode::OK,
        cache,
        etag,
    }
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

/// A query context over one native capture, with the same capture serving
/// derived index blocks.
pub(crate) fn query_context(
    dataset: Arc<NativeDataset>,
    sources: u32,
    synthetic_demo: bool,
) -> kronika_query::QueryContext {
    kronika_query::QueryContext::new(
        Arc::<NativeDataset>::clone(&dataset),
        sources,
        synthetic_demo,
    )
    .with_index_provider(dataset)
}

#[cfg(test)]
#[path = "tests/api.rs"]
mod tests;
