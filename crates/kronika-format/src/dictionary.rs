//! In-memory dictionaries for one segment.
//!
//! Each value gets a [`StrId`] and one stored copy in `dict.strings` or
//! `dict.blobs`. `dict.hot_strings` duplicates selected short strings for
//! labels that readers must resolve cheaply.
//!
//! Placement combines size, registry-forced blob placement, and strict or
//! optional hot-cache requests. Call order must not change the final result.
//!
//! The core invariants are:
//!
//! - every issued [`StrId`] resolves inside the same [`SegmentDicts`]; a
//!   segment replayed from a damaged journal may violate this when the frame
//!   carrying an id's dictionary entry was lost — readers treat such ids as
//!   unresolvable rather than failing;
//! - one id is never stored in both `dict.strings` and `dict.blobs`;
//! - `dict.hot_strings` is always a subset of `dict.strings`;
//! - truncated values keep `str_id`, `full_len`, and `full_sha256` for the
//!   full original value, not only for the stored prefix.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::StrId;

mod entry;
mod limits;

pub use entry::{BlobEntry, DictStats, EntrySnapshot, HotMark, Placement, Resolved};
use entry::{Requirements, Stored, id_or_collision};
pub use limits::{DictError, DictLimits, InvalidLimits};

/// Default boundary between `dict.strings` and `dict.blobs`, bytes.
///
/// Starting value. Finalize it after measuring real segment data.
pub const DEFAULT_BLOB_THRESHOLD: usize = 4 * 1024;

/// Default truncation limit for large values, bytes. A starting value,
/// not a fixed format constant.
pub const DEFAULT_TRUNCATE_LIMIT: usize = 1024 * 1024;

/// Default cap on the total stored bytes of one dictionary set. A starting
/// value sized to match the default `active.wal` frame limit: a window
/// that hits this cap is flushed into one journal part.
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// The dictionaries of one segment.
///
/// `intern*` methods deduplicate: the same bytes yield the same [`StrId`] and
/// one stored copy. Failed calls leave the dictionaries unchanged.
///
/// Total stored bytes are capped by [`DictLimits::max_total_bytes`]. A new
/// value past the cap returns [`DictError::Full`], the signal to flush.
/// Strict-hot values are exempt because the registry bounds their number.
#[derive(Debug, Default)]
pub struct SegmentDicts {
    limits: DictLimits,
    entries: BTreeMap<StrId, Stored>,
    /// Total stored bytes across both dictionaries; enforced against
    /// `limits.max_total_bytes`.
    stored_bytes: usize,
}

impl SegmentDicts {
    /// Create empty dictionaries with the given limits.
    #[must_use]
    pub const fn new(limits: DictLimits) -> Self {
        Self {
            limits,
            entries: BTreeMap::new(),
            stored_bytes: 0,
        }
    }

    /// Total stored bytes across both dictionaries.
    #[must_use]
    pub const fn stored_bytes(&self) -> usize {
        self.stored_bytes
    }

    /// Return the limits used by this dictionary set.
    #[must_use]
    pub const fn limits(&self) -> DictLimits {
        self.limits
    }

    /// Intern a value using size-based placement.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] if the computed id is already used
    /// for a different value, or if the input hashes to the reserved zero id.
    /// On error, dictionaries are left unchanged.
    pub fn intern(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        self.insert(bytes, Requirements::default())
    }

    /// Intern a value that must be stored in `dict.blobs` regardless of size.
    ///
    /// Use this for values whose registry entry forces blob placement.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] as in [`Self::intern`].
    /// Returns [`DictError::PlacementConflict`] if the same value is already
    /// required in `dict.hot_strings`. On error, dictionaries are left
    /// unchanged.
    pub fn intern_blob(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        self.insert(
            bytes,
            Requirements {
                blob: true,
                ..Requirements::default()
            },
        )
    }

    /// Intern a value that must be available in `dict.hot_strings`.
    ///
    /// Use this only for short values such as chart headers.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] as in [`Self::intern`].
    /// Returns [`DictError::PlacementConflict`] if size or registry
    /// requirements place the value in `dict.blobs`. On error, dictionaries
    /// are left unchanged.
    pub fn intern_hot(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        self.insert(
            bytes,
            Requirements {
                hot_hard: true,
                ..Requirements::default()
            },
        )
    }

    /// Intern a value and try to add it to `dict.hot_strings`.
    ///
    /// Returns the id and a boolean that is `true` when the value is present in
    /// `dict.hot_strings` after the call. Large values and blob-forced values
    /// keep their normal placement and return `false`.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] as in [`Self::intern`]. On error,
    /// dictionaries are left unchanged.
    pub fn intern_hot_best_effort(&mut self, bytes: &[u8]) -> Result<(StrId, bool), DictError> {
        let id = self.insert(
            bytes,
            Requirements {
                hot_soft: true,
                ..Requirements::default()
            },
        )?;
        let hot = self.entries.get(&id).is_some_and(Stored::is_hot);
        Ok((id, hot))
    }

    /// Resolve an id issued by this set.
    #[must_use]
    pub fn resolve(&self, id: StrId) -> Option<Resolved<'_>> {
        let stored = self.entries.get(&id)?;
        Some(if stored.is_blob() {
            Resolved::Blob(Self::blob_entry(id, stored))
        } else {
            Resolved::Str(&stored.bytes)
        })
    }

    /// Return the number of interned values across `dict.strings` and
    /// `dict.blobs`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been interned yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate `dict.strings` rows in on-disk sort order.
    pub fn strings(&self) -> impl Iterator<Item = (StrId, &[u8])> {
        self.entries
            .iter()
            .filter(|(_, stored)| !stored.is_blob())
            .map(|(id, stored)| (*id, stored.bytes.as_slice()))
    }

    /// Iterate `dict.blobs` rows in on-disk sort order.
    pub fn blobs(&self) -> impl Iterator<Item = BlobEntry<'_>> {
        self.entries
            .iter()
            .filter(|(_, stored)| stored.is_blob())
            .map(|(id, stored)| Self::blob_entry(*id, stored))
    }

    /// Iterate `dict.hot_strings` rows in on-disk sort order.
    ///
    /// Every returned id is also present in [`Self::strings`].
    pub fn hot_strings(&self) -> impl Iterator<Item = (StrId, &[u8])> {
        self.entries
            .iter()
            .filter(|(_, stored)| stored.is_hot())
            .map(|(id, stored)| (*id, stored.bytes.as_slice()))
    }

    /// Per-entry snapshots in `str_id` order, for the writer.
    pub fn entries(&self) -> impl Iterator<Item = EntrySnapshot<'_>> {
        self.entries.iter().map(|(id, stored)| EntrySnapshot {
            str_id: *id,
            stored_bytes: &stored.bytes,
            full_len: stored.full_len as u64,
            truncated: stored.full_sha256.is_some(),
            full_sha256: stored.full_sha256,
            placement: if stored.is_blob() {
                Placement::Blobs
            } else {
                Placement::Strings
            },
            hot: if stored.req.hot_hard {
                HotMark::Hard
            } else if stored.req.hot_soft {
                HotMark::Soft
            } else {
                HotMark::None
            },
            blob_required: stored.req.blob,
        })
    }

    /// Return current dictionary sizes.
    #[must_use]
    pub fn stats(&self) -> DictStats {
        let mut stats = DictStats::default();
        for stored in self.entries.values() {
            let stored_len = stored.bytes.len() as u64;
            if stored.is_blob() {
                stats.blob_count += 1;
                stats.truncated_blob_count += usize::from(stored.full_sha256.is_some());
                stats.blob_bytes += stored_len;
            } else {
                stats.string_count += 1;
                stats.string_bytes += stored_len;
                if stored.is_hot() {
                    stats.hot_count += 1;
                }
            }
        }
        stats
    }

    fn blob_entry(id: StrId, stored: &Stored) -> BlobEntry<'_> {
        BlobEntry {
            str_id: id,
            stored_bytes: &stored.bytes,
            full_len: stored.full_len as u64,
            truncated: stored.full_sha256.is_some(),
            full_sha256: stored.full_sha256,
        }
    }

    fn insert(&mut self, bytes: &[u8], req: Requirements) -> Result<StrId, DictError> {
        let id = id_or_collision(StrId::of(bytes))?;
        self.try_insert(id, bytes, req)
    }

    /// The single insertion path. Public `intern*` methods only hash and
    /// delegate here, so the zero-id and collision rules are testable
    /// without a known xxh3 preimage.
    ///
    /// All checks run before any mutation: a returned error leaves the
    /// dictionaries exactly as they were.
    fn try_insert(
        &mut self,
        id: StrId,
        bytes: &[u8],
        req: Requirements,
    ) -> Result<StrId, DictError> {
        if let Some(existing) = self.entries.get(&id) {
            Self::check_same_value(id, existing, bytes)?;
            let merged = Requirements {
                blob: existing.req.blob || req.blob,
                hot_hard: existing.req.hot_hard || req.hot_hard,
                hot_soft: existing.req.hot_soft || req.hot_soft,
            };
            if merged.hot_hard && (merged.blob || existing.oversized) {
                return Err(DictError::PlacementConflict { id });
            }
            // get_mut only after every check has passed.
            if let Some(stored) = self.entries.get_mut(&id) {
                stored.req = merged;
            }
            return Ok(id);
        }

        let oversized = bytes.len() >= self.limits.blob_threshold;
        if req.hot_hard && (req.blob || oversized) {
            return Err(DictError::PlacementConflict { id });
        }
        // The total-bytes cap is checked before any mutation. Strict-hot
        // values are exempt: the hot contract requires them in every part
        // and the registry bounds their number, so rejecting one here
        // would break a contract to protect a budget it cannot threaten.
        let stored_len = bytes.len().min(self.limits.truncate_limit);
        if !req.hot_hard
            && self.stored_bytes.saturating_add(stored_len) > self.limits.max_total_bytes
        {
            return Err(DictError::Full {
                stored_bytes: self.stored_bytes,
                max: self.limits.max_total_bytes,
            });
        }
        let truncated = bytes.len() > self.limits.truncate_limit;
        let stored = Stored {
            bytes: if truncated {
                bytes[..self.limits.truncate_limit].to_vec()
            } else {
                bytes.to_vec()
            },
            full_len: bytes.len(),
            full_sha256: truncated.then(|| Sha256::digest(bytes).into()),
            oversized,
            req,
        };
        self.stored_bytes += stored_len;
        self.entries.insert(id, stored);
        Ok(id)
    }

    /// Collision check between an existing entry and a re-interned value.
    ///
    /// For a truncated entry the original bytes are gone, so equality is
    /// checked via `(full_len, full_sha256)`; comparing against the
    /// stored prefix would call every repeat of the same oversized value
    /// a collision.
    fn check_same_value(id: StrId, existing: &Stored, bytes: &[u8]) -> Result<(), DictError> {
        let same = existing.full_len == bytes.len()
            && existing.full_sha256.map_or_else(
                || existing.bytes == bytes,
                |sha| <[u8; 32]>::from(Sha256::digest(bytes)) == sha,
            );
        if same {
            Ok(())
        } else {
            Err(DictError::Collision { id: id.get() })
        }
    }
}

#[cfg(test)]
mod tests;
