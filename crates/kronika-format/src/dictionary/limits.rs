//! The size bounds one segment's dictionary lives inside.

use super::{
    DEFAULT_BLOB_THRESHOLD, DEFAULT_MAX_TOTAL_BYTES, DEFAULT_TRUNCATE_LIMIT, Error, StrId, fmt,
};

/// Size limits used while building dictionaries.
///
/// Values shorter than `blob_threshold` go to `dict.strings`. Values at or
/// above that threshold go to `dict.blobs`. Values longer than
/// `truncate_limit` keep only a prefix in the segment. The total stored
/// bytes of the set are capped by `max_total_bytes`: a new value past the
/// cap fails with [`DictError::Full`], the signal to flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DictLimits {
    /// Values shorter than this go to `dict.strings`, the rest to
    /// `dict.blobs`.
    pub(super) blob_threshold: usize,
    /// Values longer than this keep only a prefix of exactly this length
    /// in the segment (`dict.blobs` truncation).
    pub(super) truncate_limit: usize,
    /// Cap on the total stored bytes across both dictionaries.
    pub(super) max_total_bytes: usize,
}

impl DictLimits {
    /// Build dictionary limits with the default total-bytes cap.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidLimits`] unless `0 < blob_threshold <=
    /// truncate_limit <= max_total_bytes`.
    pub const fn new(blob_threshold: usize, truncate_limit: usize) -> Result<Self, InvalidLimits> {
        Self::validate(Self {
            blob_threshold,
            truncate_limit,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        })
    }

    /// Replace the total-bytes cap.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidLimits`] if the cap is smaller than
    /// `truncate_limit`: the set must always be able to hold at least one
    /// value of the maximum stored size, or a single large value could
    /// never be interned at all.
    pub const fn with_max_total_bytes(self, max_total_bytes: usize) -> Result<Self, InvalidLimits> {
        Self::validate(Self {
            max_total_bytes,
            ..self
        })
    }

    pub(super) const fn validate(limits: Self) -> Result<Self, InvalidLimits> {
        if limits.blob_threshold == 0
            || limits.blob_threshold > limits.truncate_limit
            || limits.truncate_limit > limits.max_total_bytes
        {
            return Err(InvalidLimits {
                blob_threshold: limits.blob_threshold,
                truncate_limit: limits.truncate_limit,
                max_total_bytes: limits.max_total_bytes,
            });
        }
        Ok(limits)
    }

    /// The `dict.strings` / `dict.blobs` boundary, bytes.
    #[must_use]
    pub const fn blob_threshold(self) -> usize {
        self.blob_threshold
    }

    /// The truncation limit for large values, bytes.
    #[must_use]
    pub const fn truncate_limit(self) -> usize {
        self.truncate_limit
    }

    /// The cap on total stored bytes, bytes.
    #[must_use]
    pub const fn max_total_bytes(self) -> usize {
        self.max_total_bytes
    }
}

impl Default for DictLimits {
    fn default() -> Self {
        Self {
            blob_threshold: DEFAULT_BLOB_THRESHOLD,
            truncate_limit: DEFAULT_TRUNCATE_LIMIT,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

/// Rejected [`DictLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLimits {
    /// The rejected `dict.strings` / `dict.blobs` boundary.
    pub blob_threshold: usize,
    /// The rejected truncation limit.
    pub truncate_limit: usize,
    /// The rejected total-bytes cap.
    pub max_total_bytes: usize,
}

impl fmt::Display for InvalidLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dictionary limits must satisfy 0 < blob_threshold <= truncate_limit \
             <= max_total_bytes, got {}, {} and {}",
            self.blob_threshold, self.truncate_limit, self.max_total_bytes
        )
    }
}

impl Error for InvalidLimits {}

/// Why a dictionary update was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictError {
    /// One `str_id` was assigned to different values.
    ///
    /// The writer must abandon the current segment. `id == 0` means the
    /// input hashed to the reserved zero id.
    Collision {
        /// The contested raw id; `0` for the zero-hash case.
        id: u64,
    },
    /// One value has incompatible placement requirements.
    ///
    /// A value cannot be required in `dict.blobs` and in `dict.hot_strings`,
    /// because hot strings must also be present in `dict.strings`.
    PlacementConflict {
        /// The id whose requirements cannot all be satisfied.
        id: StrId,
    },
    /// Storing a new value would push the total stored bytes past
    /// [`DictLimits::max_total_bytes`].
    ///
    /// This is flow control, not corruption: the writer should flush the
    /// window into a journal part and retry. Strict-hot values are exempt —
    /// they are registry-bounded by contract and must reach every part.
    Full {
        /// Total stored bytes already held.
        stored_bytes: usize,
        /// The configured cap.
        max: usize,
    },
}

impl fmt::Display for DictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collision { id: 0 } => {
                write!(
                    f,
                    "input hashed to the reserved zero str_id, treated as a collision"
                )
            }
            Self::Collision { id } => {
                write!(
                    f,
                    "str_id collision: {id:#018x} maps to different byte values"
                )
            }
            Self::PlacementConflict { id } => {
                write!(
                    f,
                    "str_id {:#018x} is required both in dict.blobs and in dict.hot_strings",
                    id.get()
                )
            }
            Self::Full { stored_bytes, max } => {
                write!(
                    f,
                    "dictionaries hold {stored_bytes} bytes of the {max} cap; flush the window"
                )
            }
        }
    }
}

impl Error for DictError {}
