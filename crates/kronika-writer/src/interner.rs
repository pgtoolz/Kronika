//! Per-segment string interner.
//!
//! The interner keeps enough bytes to build ZMS parts without retaining every
//! string until segment completion. It has two stores:
//!
//! - the **window**: full bytes for values first seen since the previous flush;
//! - **flushed entries**: compact records for ids already written to the
//!   journal. Original bytes are not kept.
//!
//! At write time, final dictionaries are rebuilt from journaled part dictionaries
//! and the remaining window. Two cases need extra handling:
//!
//! - strict-hot values stay in memory and enter every window;
//! - a stronger requirement for a flushed value re-enters it into the window.

use std::collections::{BTreeMap, HashMap};

use kronika_format::{DictError, DictLimits, DictStats, HotMark, Placement, SegmentDicts, StrId};
use sha2::{Digest, Sha256};

/// Value metadata retained after its bytes have been written to the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Flushed {
    /// Length of the full original value, bytes.
    full_len: u64,
    /// First 16 bytes of SHA-256 of the full original value.
    ///
    /// This verifies repeated values after the full bytes have been flushed.
    check: [u8; 16],
    /// The registry forced this value into `dict.blobs`.
    blob_required: bool,
    /// Strict hot requirement.
    hot_hard: bool,
    /// Soft hot request.
    hot_soft: bool,
}

impl Flushed {
    /// Whether `bytes` is the same value this entry was created from.
    fn matches(&self, bytes: &[u8]) -> bool {
        self.full_len == bytes.len() as u64 && check16(bytes) == self.check
    }

    const fn placement(&self, limits: DictLimits) -> Placement {
        if self.blob_required || self.full_len >= limits.blob_threshold() as u64 {
            Placement::Blobs
        } else {
            Placement::Strings
        }
    }

    const fn hot(&self) -> HotMark {
        if self.hot_hard {
            HotMark::Hard
        } else if self.hot_soft {
            HotMark::Soft
        } else {
            HotMark::None
        }
    }
}

fn stats_for_flushed(entry: Flushed, limits: DictLimits) -> DictStats {
    let stored_len = entry.full_len.min(limits.truncate_limit() as u64);
    match entry.placement(limits) {
        Placement::Blobs => DictStats {
            blob_count: 1,
            truncated_blob_count: usize::from(entry.full_len > limits.truncate_limit() as u64),
            blob_bytes: stored_len,
            ..DictStats::default()
        },
        Placement::Strings => DictStats {
            string_count: 1,
            hot_count: usize::from(entry.hot() != HotMark::None),
            string_bytes: stored_len,
            ..DictStats::default()
        },
    }
}

const fn add_stats(stats: &mut DictStats, added: DictStats) {
    stats.string_count += added.string_count;
    stats.blob_count += added.blob_count;
    stats.truncated_blob_count += added.truncated_blob_count;
    stats.hot_count += added.hot_count;
    stats.string_bytes += added.string_bytes;
    stats.blob_bytes += added.blob_bytes;
}

const fn remove_stats(stats: &mut DictStats, removed: DictStats) {
    stats.string_count -= removed.string_count;
    stats.blob_count -= removed.blob_count;
    stats.truncated_blob_count -= removed.truncated_blob_count;
    stats.hot_count -= removed.hot_count;
    stats.string_bytes -= removed.string_bytes;
    stats.blob_bytes -= removed.blob_bytes;
}

/// One interning request, mirroring the four `intern*` entry points.
#[derive(Debug, Clone, Copy, Default)]
struct Request {
    blob_required: bool,
    hot_hard: bool,
    hot_soft: bool,
}

/// First 16 bytes of SHA-256 over `bytes`.
fn check16(bytes: &[u8]) -> [u8; 16] {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let mut out = [0_u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// First 16 bytes of an already computed SHA-256.
fn first16(digest: [u8; 32]) -> [u8; 16] {
    let mut out = [0_u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// Data returned when the interner finishes a segment.
#[derive(Debug)]
pub struct FinishedSegment {
    /// Values still in memory, not yet written to the journal.
    ///
    /// Segment completion merges this window with journal parts.
    pub window: SegmentDicts,
    /// Final placement directives for flushed ids, in `str_id` order.
    pub flushed: Vec<FlushedEntry>,
}

/// Placement directive for one flushed id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushedEntry {
    /// The id.
    pub str_id: StrId,
    /// Length of the full original value, bytes.
    pub full_len: u64,
    /// Final dictionary for the value.
    pub placement: Placement,
    /// The hot requirement accumulated for the value.
    pub hot: HotMark,
}

/// Per-segment string interner.
///
/// All `intern*` methods deduplicate against both the window and flushed
/// entries. A failed call changes no state. Repeats of already-flushed values
/// do not enter the window again unless they add a stronger placement
/// requirement.
#[derive(Debug)]
pub struct Interner {
    window: SegmentDicts,
    /// Identities of values already in the journal: ~48 bytes per distinct
    /// id (plus map overhead) until `write_segment()`. The journal cap
    /// (`JournalError::Full`) forces a merge before the journal, and with it
    /// this map, can grow without limit.
    flushed: HashMap<StrId, Flushed>,
    /// Aggregate for values already flushed into the journal.
    flushed_stats: DictStats,
    /// Bytes of strict-hot values inserted into every window.
    ///
    /// Bounded by contract, not by data: callers may strict-hot only
    /// registry-defined header strings (chart headers, the catalog
    /// required catalog labels), each shorter than `blob_threshold`. A data-driven
    /// strict-hot call is a caller bug.
    hot_pinned: BTreeMap<StrId, Vec<u8>>,
}

impl Interner {
    /// Create an empty interner for a new segment.
    #[must_use]
    pub fn new(limits: DictLimits) -> Self {
        Self {
            window: SegmentDicts::new(limits),
            flushed: HashMap::new(),
            flushed_stats: DictStats::default(),
            hot_pinned: BTreeMap::new(),
        }
    }

    /// Intern with size-based placement.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] if the id is already used for a
    /// different value, or if the input hashes to zero. On error, the interner
    /// state is unchanged.
    pub fn intern(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        self.request(bytes, Request::default()).map(|(id, _)| id)
    }

    /// Intern a value that must be stored in `dict.blobs`.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] or [`DictError::PlacementConflict`] as
    /// in [`SegmentDicts::intern_blob`]. On error, the interner state is
    /// unchanged.
    pub fn intern_blob(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        self.request(
            bytes,
            Request {
                blob_required: true,
                ..Request::default()
            },
        )
        .map(|(id, _)| id)
    }

    /// Intern a value that must be available in every part hot cache.
    ///
    /// The value is kept in memory and inserted into each new window.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] or [`DictError::PlacementConflict`] as
    /// in [`SegmentDicts::intern_hot`]. On error, the interner state is
    /// unchanged.
    pub fn intern_hot(&mut self, bytes: &[u8]) -> Result<StrId, DictError> {
        let (id, _) = self.request(
            bytes,
            Request {
                hot_hard: true,
                ..Request::default()
            },
        )?;
        self.hot_pinned.entry(id).or_insert_with(|| bytes.to_vec());
        Ok(id)
    }

    /// Intern a value and try to add it to `dict.hot_strings`.
    ///
    /// Returns the id and whether the value is hot after this call. Large or
    /// blob-forced values keep their normal placement and return `false`.
    ///
    /// # Errors
    ///
    /// Returns [`DictError::Collision`] as in [`Self::intern`]. On error, the
    /// interner state is unchanged.
    pub fn intern_hot_best_effort(&mut self, bytes: &[u8]) -> Result<(StrId, bool), DictError> {
        self.request(
            bytes,
            Request {
                hot_soft: true,
                ..Request::default()
            },
        )
    }

    /// Flush the current window to the journal.
    ///
    /// Entries move to compact records only after `write` returns `Ok`. On
    /// error, the window is left unchanged.
    ///
    /// Returns the number of entries flushed.
    ///
    /// # Errors
    ///
    /// Returns whatever `write` returns.
    pub fn flush_window<E>(
        &mut self,
        write: impl FnOnce(&SegmentDicts) -> Result<(), E>,
    ) -> Result<usize, E> {
        write(&self.window)?;

        let count = self.window.len();
        let limits = self.window.limits();
        for snap in self.window.entries() {
            // The stored bytes are the full value when not truncated.
            let check = snap
                .full_sha256
                .map_or_else(|| check16(snap.stored_bytes), first16);
            let value = Flushed {
                full_len: snap.full_len,
                check,
                blob_required: snap.blob_required,
                hot_hard: snap.hot == HotMark::Hard,
                hot_soft: snap.hot == HotMark::Soft,
            };
            if let Some(previous) = self.flushed.insert(snap.str_id, value) {
                remove_stats(&mut self.flushed_stats, stats_for_flushed(previous, limits));
            }
            add_stats(&mut self.flushed_stats, stats_for_flushed(value, limits));
        }

        self.window = SegmentDicts::new(limits);
        // Reinsert strict-hot values so the next part carries them too.
        // These re-inserts cannot fail: every pinned value already passed
        // the strict-hot checks once, and the window is empty.
        for bytes in self.hot_pinned.values() {
            let _ = self.window.intern_hot(bytes);
        }
        Ok(count)
    }

    /// Finish the segment and reset the interner for the next segment.
    ///
    /// Returns the remaining window plus placement directives for values already
    /// flushed to the journal.
    pub fn write_segment(&mut self) -> FinishedSegment {
        let limits = self.window.limits();
        let window = std::mem::replace(&mut self.window, SegmentDicts::new(limits));
        let flushed = std::mem::take(&mut self.flushed);
        self.flushed_stats = DictStats::default();
        self.hot_pinned.clear();

        let mut entries: Vec<FlushedEntry> = flushed
            .iter()
            .map(|(id, f)| FlushedEntry {
                str_id: *id,
                full_len: f.full_len,
                placement: f.placement(limits),
                hot: f.hot(),
            })
            .collect();
        entries.sort_by_key(|entry| entry.str_id);

        FinishedSegment {
            window,
            flushed: entries,
        }
    }

    /// Return dictionary sizes across the window and flushed entries.
    ///
    /// Byte sizes of flushed values count the stored form after truncation,
    /// matching what is on disk.
    #[must_use]
    pub fn stats(&self) -> DictStats {
        let limits = self.window.limits();
        let mut stats = self.flushed_stats;
        add_stats(&mut stats, self.window.stats());
        for snapshot in self.window.entries() {
            if let Some(previous) = self.flushed.get(&snapshot.str_id) {
                remove_stats(&mut stats, stats_for_flushed(*previous, limits));
            }
        }
        stats
    }

    /// Return the current window.
    #[must_use]
    pub const fn window(&self) -> &SegmentDicts {
        &self.window
    }

    /// Return whether the id was interned in this segment.
    #[must_use]
    pub fn is_interned(&self, id: StrId) -> bool {
        self.window.resolve(id).is_some() || self.flushed.contains_key(&id)
    }

    /// Shared intern path.
    ///
    /// Checks the window first, then the flushed map, then inserts into the
    /// window. All checks run before any mutation.
    fn request(&mut self, bytes: &[u8], req: Request) -> Result<(StrId, bool), DictError> {
        let Some(id) = StrId::of(bytes) else {
            return Err(DictError::Collision { id: 0 });
        };

        if self.window.resolve(id).is_some() {
            return self.apply_to_window(bytes, req);
        }

        if let Some(flushed) = self.flushed.get(&id) {
            if !flushed.matches(bytes) {
                return Err(DictError::Collision { id: id.get() });
            }
            let merged = Request {
                blob_required: flushed.blob_required || req.blob_required,
                hot_hard: flushed.hot_hard || req.hot_hard,
                hot_soft: flushed.hot_soft || req.hot_soft,
            };
            let oversized = flushed.full_len >= self.window.limits().blob_threshold() as u64;
            if merged.hot_hard && (merged.blob_required || oversized) {
                return Err(DictError::PlacementConflict { id });
            }
            let placement_is_blob = merged.blob_required || oversized;
            // Only changes that must survive a crash re-enter the window:
            // placement (forced blob) and the strict hot mark are rebuilt
            // from part dictionaries at recovery, so the next part has to
            // record them. A soft hot mark may be lost after a crash. On a
            // blob-placed value it never becomes effective, so neither case
            // is worth loading a large value into memory again.
            let durable_upgrade = merged.blob_required != flushed.blob_required
                || merged.hot_hard != flushed.hot_hard;
            let soft_became_effective = merged.hot_soft != flushed.hot_soft && !placement_is_blob;

            if durable_upgrade {
                let result = self.apply_to_window(bytes, merged)?;
                self.record_flushed_bits(id, merged);
                return Ok(result);
            }
            if soft_became_effective {
                self.record_flushed_bits(id, merged);
            }
            // The common case: a repeat of a flushed value does not
            // re-enter memory.
            let hot = (merged.hot_hard || merged.hot_soft) && !placement_is_blob;
            return Ok((id, hot));
        }

        self.apply_to_window(bytes, req)
    }

    /// Keep the flushed record in sync with an accepted upgrade, so
    /// [`Interner::close`] can report the final directives even before the next
    /// flush writes the upgraded value again.
    fn record_flushed_bits(&mut self, id: StrId, merged: Request) {
        if let Some(entry) = self.flushed.get_mut(&id) {
            let previous = *entry;
            entry.blob_required = merged.blob_required;
            entry.hot_hard = merged.hot_hard;
            entry.hot_soft = merged.hot_soft;
            let updated = *entry;
            let limits = self.window.limits();
            remove_stats(&mut self.flushed_stats, stats_for_flushed(previous, limits));
            add_stats(&mut self.flushed_stats, stats_for_flushed(updated, limits));
        }
    }

    /// Apply a request to the window, bit by bit: requirements
    /// accumulate inside [`SegmentDicts`], so each flag is one call.
    fn apply_to_window(&mut self, bytes: &[u8], req: Request) -> Result<(StrId, bool), DictError> {
        // Pre-check the conflict so that a multi-flag request cannot
        // fail halfway and leave a partially-required entry behind.
        let oversized = bytes.len() >= self.window.limits().blob_threshold();
        if req.hot_hard && (req.blob_required || oversized) {
            return Err(DictError::PlacementConflict {
                id: StrId::of(bytes).unwrap_or_else(|| unreachable!("checked by request()")),
            });
        }

        let id = self.window.intern(bytes)?;
        if req.blob_required {
            self.window.intern_blob(bytes)?;
        }
        if req.hot_hard {
            self.window.intern_hot(bytes)?;
        }
        let hot = if req.hot_soft {
            self.window.intern_hot_best_effort(bytes)?.1
        } else {
            // A successful hard request is hot by definition; the other
            // callers discard the flag, so no window scan is needed.
            req.hot_hard
        };
        Ok((id, hot))
    }
}

#[cfg(test)]
mod tests;
