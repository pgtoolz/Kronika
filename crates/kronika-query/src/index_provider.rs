//! Injected access to derived index blocks.

use std::sync::Arc;

use kronika_index::{Index, IndexError, SeriesKey, TargetedIndex};
use kronika_layout::SegmentId;
use kronika_reader::SegmentKind;

use crate::{DatasetSegment, QueryError};

/// Selected derived blocks for one captured segment.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexResource {
    /// Validated selected blocks.
    pub index: TargetedIndex,
}

/// Small boundary around loading derived index blocks.
pub trait IndexProvider: std::fmt::Debug + Send + Sync {
    /// Load only the selected derived blocks for one exact captured segment.
    ///
    /// # Errors
    ///
    /// Returns a captured-source, decoding, build, or cache error.
    fn load(
        &self,
        segment: &DatasetSegment,
        logical_name: &str,
        keys: &[SeriesKey],
    ) -> Result<IndexResource, QueryError>;
}

struct EncodedIndex(Vec<u8>);

/// One validated encoded IDX bound to one finished segment in memory.
///
/// This provider only selects blocks from exporter-supplied canonical bytes.
/// It does not build missing data or infer predecessor state. A report exporter
/// may provide a predecessor-aware IDX; an isolated-WASM builder is a separate
/// future concern.
///
/// IDX bytes carry no segment or ZMS identity. The explicit `SegmentId`
/// binding supplied by the exporter is authoritative.
#[derive(Clone)]
pub struct MemoryIndexProvider {
    segment_id: SegmentId,
    bytes: Arc<EncodedIndex>,
}

impl std::fmt::Debug for MemoryIndexProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryIndexProvider")
            .field("segment_id", &self.segment_id)
            .field("len", &self.bytes.0.len())
            .field("capacity", &self.bytes.0.capacity())
            .finish()
    }
}

impl MemoryIndexProvider {
    /// Validate and retain one current encoded IDX without copying its buffer.
    ///
    /// # Errors
    ///
    /// Returns the current-format size, framing, checksum, or block error.
    pub fn new(segment_id: SegmentId, bytes: Vec<u8>) -> Result<Self, IndexError> {
        Index::decode(&bytes)?;
        Ok(Self {
            segment_id,
            bytes: Arc::new(EncodedIndex(bytes)),
        })
    }
}

impl IndexProvider for MemoryIndexProvider {
    fn load(
        &self,
        segment: &DatasetSegment,
        logical_name: &str,
        keys: &[SeriesKey],
    ) -> Result<IndexResource, QueryError> {
        if segment.id() != self.segment_id.get() {
            return Err(QueryError::NoSuchSegment);
        }
        if segment.kind() != SegmentKind::Finished {
            return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "memory indexes require a finished segment",
            ))));
        }
        let index = Index::decode_target(&self.bytes.0, keys)
            .map_err(|error| QueryError::Unreadable(Box::new(error)))?;
        if !index.contains_targets(keys) {
            return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("memory index has no required {logical_name} block"),
            ))));
        }
        Ok(IndexResource { index })
    }
}

#[cfg(test)]
#[path = "index_provider/tests.rs"]
mod tests;
