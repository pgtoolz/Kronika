use crate::{LayoutError, LimitKind};

const HARD_MAX_VISITED_ENTRIES: usize = 4_000_000;
const HARD_MAX_ENTRIES_PER_DAY: usize = 1_000_000;
const HARD_MAX_SEGMENTS: usize = 2_000_000;
const HARD_MAX_METADATA_BYTES: usize = 128 * 1024 * 1024;

/// Non-zero hard-capped resource limits for one strict tree traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_field_names,
    reason = "`LayoutLimits::max_*` makes each public bound explicit at call sites"
)]
pub struct LayoutLimits {
    /// Maximum number of entries visited across the entire tree.
    pub max_visited_entries: usize,
    /// Maximum number of entries visited in a single day directory.
    pub max_entries_per_day: usize,
    /// Maximum number of finished ZMS segments returned.
    pub max_segments: usize,
    /// Maximum accounted bytes for names and result metadata.
    pub max_metadata_bytes: usize,
}

impl LayoutLimits {
    /// Validates all limits against their non-zero hard ranges.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::InvalidLimits`] for a zero or excessive value.
    pub fn validate(self) -> Result<Self, LayoutError> {
        validate_limit(
            LimitKind::VisitedEntries,
            self.max_visited_entries,
            HARD_MAX_VISITED_ENTRIES,
        )?;
        validate_limit(
            LimitKind::EntriesPerDay,
            self.max_entries_per_day,
            HARD_MAX_ENTRIES_PER_DAY,
        )?;
        validate_limit(LimitKind::Segments, self.max_segments, HARD_MAX_SEGMENTS)?;
        validate_limit(
            LimitKind::MetadataBytes,
            self.max_metadata_bytes,
            HARD_MAX_METADATA_BYTES,
        )?;
        Ok(self)
    }
}

impl Default for LayoutLimits {
    fn default() -> Self {
        Self {
            max_visited_entries: 1_000_000,
            max_entries_per_day: 10_000,
            max_segments: 500_000,
            max_metadata_bytes: HARD_MAX_METADATA_BYTES,
        }
    }
}

const fn validate_limit(kind: LimitKind, value: usize, hard_max: usize) -> Result<(), LayoutError> {
    if value == 0 || value > hard_max {
        Err(LayoutError::InvalidLimits {
            kind,
            value,
            hard_max,
        })
    } else {
        Ok(())
    }
}
