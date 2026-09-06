//! Captured segment inventory and opaque adapter handles.

use std::any::Any;
use std::fmt::Debug;
use std::ops::Bound;
use std::sync::Arc;

use kronika_reader::{Segment, SegmentKind, SegmentSection};

use crate::QueryError;

/// Adapter-owned handle carried without interpretation by query code.
#[derive(Clone)]
pub struct OpaqueCapture(Arc<dyn Any + Send + Sync>);

impl OpaqueCapture {
    /// Erase one adapter-specific captured handle.
    #[must_use]
    pub fn new<T: Debug + Send + Sync + 'static>(capture: T) -> Self {
        Self(Arc::new(capture))
    }

    /// Recover a handle inside the adapter that created it.
    #[must_use]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl Debug for OpaqueCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueCapture").finish_non_exhaustive()
    }
}

/// One segment from a captured catalog.
#[derive(Debug, Clone)]
pub struct DatasetSegment {
    capture: OpaqueCapture,
    id: i64,
    kind: SegmentKind,
    min_ts: i64,
    max_ts: i64,
    active_position: Option<u64>,
    sections: Arc<[SegmentSection]>,
}

impl DatasetSegment {
    /// Build a neutral descriptor around an adapter-owned capture.
    #[must_use]
    pub const fn new(
        capture: OpaqueCapture,
        id: i64,
        kind: SegmentKind,
        min_ts: i64,
        max_ts: i64,
        active_position: Option<u64>,
        sections: Arc<[SegmentSection]>,
    ) -> Self {
        Self {
            capture,
            id,
            kind,
            min_ts,
            max_ts,
            active_position,
            sections,
        }
    }

    /// Stable segment identity.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Whether the capture is finished or active.
    #[must_use]
    pub const fn kind(&self) -> SegmentKind {
        self.kind
    }

    /// Earliest timestamp carried by the capture.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.min_ts
    }

    /// Latest timestamp carried by the capture.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.max_ts
    }

    /// Committed journal position for an active capture.
    #[must_use]
    pub const fn active_position(&self) -> Option<u64> {
        self.active_position
    }

    /// Physical sections in numeric layout order.
    #[must_use]
    pub fn sections(&self) -> &[SegmentSection] {
        &self.sections
    }

    /// Adapter-owned handle associated with this descriptor.
    #[must_use]
    pub const fn capture(&self) -> &OpaqueCapture {
        &self.capture
    }
}

/// Neutral subject of a non-fatal catalog diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DatasetWarningSubject {
    /// One canonical finished segment.
    Segment(i64),
    /// The captured active journal.
    ActiveJournal,
    /// One unsupported entry, represented without its name.
    ForeignEntry {
        /// Stable hash of the unsupported name.
        name_hash: u64,
        /// Byte length of the unsupported name.
        name_len: u16,
    },
    /// Another source-owned object class.
    Other,
}

/// One bounded non-fatal catalog diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatasetWarning {
    /// Affected captured object.
    pub subject: DatasetWarningSubject,
    /// Stable low-cardinality diagnostic code.
    pub code: &'static str,
}

/// Captured segments and diagnostics in source order.
#[derive(Debug)]
pub struct DatasetListing {
    /// Canonical segments in ascending identity order.
    pub segments: Vec<DatasetSegment>,
    /// Objects excluded during discovery.
    pub warnings: Vec<DatasetWarning>,
}

/// Inclusive/exclusive bounds passed to one catalog capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentBounds {
    /// Lower timestamp bound.
    pub start: Bound<i64>,
    /// Upper timestamp bound.
    pub end: Bound<i64>,
}

impl SegmentBounds {
    /// Every recorded timestamp.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }

    /// Optional inclusive timestamp bounds.
    #[must_use]
    pub fn inclusive(from: Option<i64>, to: Option<i64>) -> Self {
        Self {
            start: from.map_or(Bound::Unbounded, Bound::Included),
            end: to.map_or(Bound::Unbounded, Bound::Included),
        }
    }

    /// Half-open timestamp bounds.
    #[must_use]
    pub const fn half_open(from: i64, to_exclusive: i64) -> Self {
        Self {
            start: Bound::Included(from),
            end: Bound::Excluded(to_exclusive),
        }
    }
}

/// Optional predecessor admission for one captured selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredecessorSelection {
    /// Select only overlapping segments.
    None,
    /// Include the closest canonical predecessor.
    Closest,
    /// Include closest predecessors carrying any requested layout.
    ForLayouts(Vec<u32>),
}

/// One selection from a single captured catalog pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSelection {
    /// Timestamp bounds.
    pub bounds: SegmentBounds,
    /// Required predecessor behavior.
    pub predecessor: PredecessorSelection,
}

impl SegmentSelection {
    /// Select overlapping segments without a predecessor.
    #[must_use]
    pub const fn new(bounds: SegmentBounds) -> Self {
        Self {
            bounds,
            predecessor: PredecessorSelection::None,
        }
    }
}

/// One catalog pass pinned by a data adapter.
pub trait CapturedCatalog: Debug {
    /// Time ranges of canonical segments in the capture.
    fn ranges(&self) -> &[(i64, i64)];

    /// Select descriptors from this same capture.
    ///
    /// # Errors
    ///
    /// Returns a captured-source error when selected metadata cannot be read.
    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError>;
}

/// Small data boundary required by recorded-data query execution.
pub trait QueryDataset: Debug + Send + Sync {
    /// Capture a catalog whose ranges and selected descriptors share one view.
    ///
    /// # Errors
    ///
    /// Returns a captured-source error when discovery cannot begin.
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError>;

    /// Select one exact segment identity.
    ///
    /// # Errors
    ///
    /// Returns a captured-source error when discovery cannot complete.
    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError>;

    /// Open the exact captured descriptor through the row decoder.
    ///
    /// # Errors
    ///
    /// Returns an error for a foreign, stale, or unreadable capture.
    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError>;

    /// Pin an active descriptor to an earlier committed journal position.
    ///
    /// # Errors
    ///
    /// Returns an error when the descriptor or position is invalid.
    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError>;
}
