//! Portable query and self-contained HTML report composition.

#[cfg(test)]
use serde_json as _;
#[cfg(feature = "generator")]
use {kronika_format as _, tempfile as _};

#[cfg(feature = "generator")]
mod generator;

#[cfg(feature = "generator")]
pub use generator::{
    HtmlReportError, HtmlReportInput, HtmlReportSummary, ReportTimeRange, write_html,
    write_html_from_file, write_html_from_file_with_range, write_html_from_file_with_segment_id,
};

use std::sync::Arc;

use kronika_index::IndexError;
use kronika_layout::{LayoutError, SegmentId};
use kronika_query::{
    FinishedDataset, MemoryIndexProvider, QueryContext, QueryError, QueryRequest, QuerySink,
};
use kronika_store::{EmbeddedSource, ResourceError};

/// Owned inputs for one standalone report engine.
#[derive(Debug)]
pub struct ReportInput {
    /// Explicit identity bound to both embedded artifacts.
    pub segment_id: SegmentId,
    /// Complete finished ZMS allocation.
    pub zms: Vec<u8>,
    /// Complete canonical IDX allocation.
    pub idx: Vec<u8>,
    /// Source-family bits presented by catalog and hour queries.
    pub configured_sources: u32,
    /// Maximum accepted logical ZMS length in bytes.
    pub max_zms_bytes: u64,
}

/// Typed failure while preparing or binding report artifacts.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReportError {
    /// Invalid segment identity preserved while preparing typed input.
    Layout(LayoutError),
    /// Invalid or over-limit finished ZMS bytes.
    Resource(ResourceError),
    /// Invalid canonical IDX bytes.
    Index(IndexError),
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Layout(error) => error.fmt(f),
            Self::Resource(error) => error.fmt(f),
            Self::Index(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Layout(error) => Some(error),
            Self::Resource(error) => Some(error),
            Self::Index(error) => Some(error),
        }
    }
}

impl From<LayoutError> for ReportError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<ResourceError> for ReportError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

impl From<IndexError> for ReportError {
    fn from(error: IndexError) -> Self {
        Self::Index(error)
    }
}

/// Synchronous query entry point for one finished embedded segment.
#[derive(Debug)]
pub struct ReportEngine {
    context: QueryContext,
}

impl ReportEngine {
    /// Bind one owned ZMS and its matching canonical IDX.
    ///
    /// # Errors
    ///
    /// Returns the original typed ZMS or IDX validation failure.
    pub fn new(input: ReportInput) -> Result<Self, ReportError> {
        let source = EmbeddedSource::from_owned(input.segment_id, input.zms, input.max_zms_bytes)?;
        let indexes = MemoryIndexProvider::new(input.segment_id, input.idx)?;
        let dataset = Arc::new(FinishedDataset::new(source));
        let context = QueryContext::new(dataset, input.configured_sources, false)
            .with_index_provider(Arc::new(indexes));
        Ok(Self { context })
    }

    /// Execute one existing typed query and stream its framed records.
    ///
    /// # Errors
    ///
    /// Returns the unchanged shared query failure.
    pub fn execute(
        &self,
        request: QueryRequest,
        sink: &mut dyn QuerySink,
    ) -> Result<(), QueryError> {
        kronika_query::execute(&self.context, request)?.stream(sink)
    }
}
