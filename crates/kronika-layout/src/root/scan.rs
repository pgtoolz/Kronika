//! The strict traversal state and the snapshot it builds.

use std::ffi::CStr;

use crate::error::{LayoutError, LimitKind};
use crate::time::{SegmentAddress, UtcDay};

use super::entry::{
    EntryDiagnostic, EntryParent, EntryPath, ForeignEntry, ForeignEntryReason, LayoutSnapshot,
    SegmentArtifacts, TemporaryKind, TemporaryObject,
};
use super::names::path_identity;
use super::{ENTRY_METADATA_BYTES, FileIdentity, LayoutLimits};

#[derive(Default)]
pub(super) struct DayArtifacts {
    pub(super) zms: Option<FileIdentity>,
    pub(super) idx: Option<u64>,
}

#[derive(Debug)]
pub(super) struct ScanState<'scan> {
    pub(super) limits: LayoutLimits,
    pub(super) visited_entries: &'scan mut usize,
    pub(super) metadata_bytes: usize,
    pub(super) days: Vec<UtcDay>,
    pub(super) segments: Vec<SegmentArtifacts>,
    pub(super) orphan_indexs: Vec<SegmentAddress>,
    pub(super) temporaries: Vec<TemporaryObject>,
    pub(super) foreign_entries: Vec<ForeignEntry>,
}

impl<'scan> ScanState<'scan> {
    pub(super) const fn new(limits: LayoutLimits, visited_entries: &'scan mut usize) -> Self {
        Self {
            limits,
            visited_entries,
            metadata_bytes: 0,
            days: Vec::new(),
            segments: Vec::new(),
            orphan_indexs: Vec::new(),
            temporaries: Vec::new(),
            foreign_entries: Vec::new(),
        }
    }

    pub(super) fn record_foreign(
        &mut self,
        parent: EntryParent,
        name: &CStr,
        stat: &rustix::fs::Stat,
        reason: ForeignEntryReason,
    ) -> Result<(), LayoutError> {
        self.account_metadata(size_of::<ForeignEntry>())?;
        let path = EntryPath {
            parent,
            name: name.to_owned(),
        };
        let diagnostic = EntryDiagnostic {
            path: path_identity(parent, name, stat),
            reason,
        };
        self.foreign_entries.push(ForeignEntry { diagnostic, path });
        Ok(())
    }

    pub(super) fn account(&mut self, name_bytes: usize) -> Result<(), LayoutError> {
        *self.visited_entries = self.visited_entries.checked_add(1).ok_or({
            LayoutError::TraversalLimitExceeded {
                kind: LimitKind::VisitedEntries,
                limit: self.limits.max_visited_entries,
            }
        })?;
        if *self.visited_entries > self.limits.max_visited_entries {
            return Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::VisitedEntries,
                limit: self.limits.max_visited_entries,
            });
        }
        self.account_metadata(name_bytes.saturating_add(ENTRY_METADATA_BYTES))
    }

    pub(super) fn account_metadata(&mut self, bytes: usize) -> Result<(), LayoutError> {
        self.metadata_bytes = self.metadata_bytes.checked_add(bytes).ok_or({
            LayoutError::TraversalLimitExceeded {
                kind: LimitKind::MetadataBytes,
                limit: self.limits.max_metadata_bytes,
            }
        })?;
        if self.metadata_bytes > self.limits.max_metadata_bytes {
            return Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::MetadataBytes,
                limit: self.limits.max_metadata_bytes,
            });
        }
        Ok(())
    }

    pub(super) fn account_segment(&mut self) -> Result<(), LayoutError> {
        if self.segments.len() >= self.limits.max_segments {
            return Err(LayoutError::TraversalLimitExceeded {
                kind: LimitKind::Segments,
                limit: self.limits.max_segments,
            });
        }
        self.account_metadata(size_of::<SegmentArtifacts>())
    }

    pub(super) fn finish(mut self) -> LayoutSnapshot {
        self.days.sort_unstable();
        self.segments.sort_by_key(|segment| segment.address.id);
        self.orphan_indexs.sort_by_key(|address| address.id);
        self.temporaries
            .sort_by_key(|temporary| (temporary.address.id, temporary.kind as u8));
        self.foreign_entries
            .sort_by_key(|entry| entry.diagnostic.path);
        LayoutSnapshot {
            days: self.days,
            segments: self.segments,
            orphan_indexs: self.orphan_indexs,
            temporaries: self.temporaries,
            foreign_entries: self.foreign_entries,
            visited_entries: *self.visited_entries,
            metadata_bytes: self.metadata_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ParsedLeaf {
    Zms(SegmentAddress),
    Idx(SegmentAddress),
    Temporary(SegmentAddress, TemporaryKind),
}
