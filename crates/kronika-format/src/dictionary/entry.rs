//! What one dictionary entry looks like to a caller.

use super::{DictError, StrId};

/// One `dict.blobs` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobEntry<'a> {
    /// Id of the full original value.
    pub str_id: StrId,
    /// Stored bytes: the full value, or its prefix when truncated.
    pub stored_bytes: &'a [u8],
    /// Length of the full original value, bytes.
    pub full_len: u64,
    /// Whether only a prefix of the value is stored.
    pub truncated: bool,
    /// SHA-256 of the full original value; present only when truncated.
    pub full_sha256: Option<[u8; 32]>,
}

/// A resolved dictionary value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved<'a> {
    /// The value is stored in `dict.strings` and kept in full.
    Str(&'a [u8]),
    /// The value is stored in `dict.blobs` and may be truncated.
    Blob(BlobEntry<'a>),
}

impl<'a> Resolved<'a> {
    /// Bytes retained in the segment for this dictionary value.
    #[must_use]
    pub const fn stored_bytes(self) -> &'a [u8] {
        match self {
            Self::Str(bytes) => bytes,
            Self::Blob(blob) => blob.stored_bytes,
        }
    }
}

/// Current dictionary placement for an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// `dict.strings`.
    Strings,
    /// `dict.blobs`.
    Blobs,
}

/// Hot-cache requirement requested for an entry.
///
/// Effective `dict.hot_strings` membership also requires
/// [`Placement::Strings`]. A soft mark on a blob-placed value leaves the value
/// out of the hot cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotMark {
    /// Never requested hot.
    None,
    /// Soft hot request (event labels).
    Soft,
    /// Strict hot (chart headers and other required small values).
    Hard,
}

/// Snapshot used by the writer when flushing a dictionary window.
#[derive(Debug, Clone, Copy)]
pub struct EntrySnapshot<'a> {
    /// Id of the full original value.
    pub str_id: StrId,
    /// Stored bytes: the full value, or its prefix when truncated.
    pub stored_bytes: &'a [u8],
    /// Length of the full original value, bytes.
    pub full_len: u64,
    /// Whether only a prefix of the value is stored.
    pub truncated: bool,
    /// SHA-256 of the full original value; present only when truncated.
    pub full_sha256: Option<[u8; 32]>,
    /// Current placement after applying the entry requirements.
    pub placement: Placement,
    /// The hot requirement requested for the entry.
    pub hot: HotMark,
    /// Whether the registry forced this value into `dict.blobs`
    /// regardless of size.
    pub blob_required: bool,
}

/// Current dictionary sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DictStats {
    /// Number of `dict.strings` entries.
    pub string_count: usize,
    /// Number of `dict.blobs` entries.
    pub blob_count: usize,
    /// Number of truncated `dict.blobs` entries.
    pub truncated_blob_count: usize,
    /// Number of `dict.hot_strings` entries.
    pub hot_count: usize,
    /// Total stored bytes of `dict.strings` values.
    pub string_bytes: u64,
    /// Total stored bytes of `dict.blobs` values (after truncation).
    pub blob_bytes: u64,
}

/// Requirements attached to one interning call.
///
/// Requirements accumulate per value and are never withdrawn. This makes final
/// placement independent of call order.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Requirements {
    /// The registry requires this value in `dict.blobs` regardless of
    /// size, e.g. query plans.
    pub(super) blob: bool,
    /// The strict part of the hot contract: the value must be readable
    /// from `dict.hot_strings` (chart headers and other required small values).
    pub(super) hot_hard: bool,
    /// Soft hot request: duplicate into `dict.hot_strings` when placement
    /// permits it; otherwise leave it out without failing.
    pub(super) hot_soft: bool,
}

/// One stored value with its accumulated requirements.
#[derive(Debug)]
pub(super) struct Stored {
    /// The full value, or its prefix of `truncate_limit` bytes.
    pub(super) bytes: Vec<u8>,
    /// Length of the full original value.
    pub(super) full_len: usize,
    /// SHA-256 of the full original value; `Some` iff truncated.
    pub(super) full_sha256: Option<[u8; 32]>,
    /// Routed to blobs by size at insert time (`full_len >= threshold`).
    pub(super) oversized: bool,
    pub(super) req: Requirements,
}

impl Stored {
    /// Whether the value is placed in `dict.blobs` (forced or oversized).
    pub(super) const fn is_blob(&self) -> bool {
        self.req.blob || self.oversized
    }

    /// Whether the value is in `dict.hot_strings`. A soft hot request on
    /// a blob-placed value does not add it to the hot cache; a hard one is
    /// rejected before getting here.
    pub(super) const fn is_hot(&self) -> bool {
        !self.is_blob() && (self.req.hot_hard || self.req.hot_soft)
    }
}

/// Convert the optional hash result into a dictionary id.
///
/// Zero is the on-disk "no value" sentinel. If `StrId::of` returns `None`,
/// the caller must treat it as a collision.
pub(super) fn id_or_collision(hashed: Option<StrId>) -> Result<StrId, DictError> {
    hashed.ok_or(DictError::Collision { id: 0 })
}
