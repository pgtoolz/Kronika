//! Count stored files and remove candidates in retention order.

use anyhow::{Context, Result};
use kronika_layout::{LayoutSnapshot, SegmentAddress, TemporaryKind, TemporaryObject, WriterOwner};

use crate::logging::{LogLevel, field, log_event};

pub(super) enum Candidate<'a> {
    Temporary(&'a TemporaryObject),
    OrphanIndex(SegmentAddress),
    Segment(SegmentAddress),
}

/// Writer temporaries first, then orphan indexes, then oldest segments.
/// The scan sorts segments by ID; its last segment and the active WAL are kept.
pub(super) fn deletion_candidates(
    snapshot: &LayoutSnapshot,
) -> impl Iterator<Item = Candidate<'_>> {
    let temporaries = snapshot
        .temporaries
        .iter()
        .filter(|temporary| temporary.kind == TemporaryKind::Zms)
        .map(Candidate::Temporary);
    let orphans = snapshot
        .orphan_indexs
        .iter()
        .copied()
        .map(Candidate::OrphanIndex);
    let segments = snapshot
        .segments
        .iter()
        .take(snapshot.segments.len().saturating_sub(1))
        .map(|segment| Candidate::Segment(segment.address));
    temporaries.chain(orphans).chain(segments)
}

/// Count segments with their indexes and all temporaries. The writer supplies
/// WAL bytes separately; the scan does not report sizes for orphan indexes.
pub(super) fn countable_bytes(snapshot: &LayoutSnapshot) -> u64 {
    let segments = snapshot.segments.iter().map(|segment| {
        segment
            .zms_bytes
            .saturating_add(segment.idx_bytes.unwrap_or(0))
    });
    let temporaries = snapshot
        .temporaries
        .iter()
        .map(|temporary| temporary.identity.len);
    segments.chain(temporaries).fold(0, u64::saturating_add)
}

impl Candidate<'_> {
    pub(super) fn remove(&self, owner: &WriterOwner) -> Result<u64> {
        match self {
            Self::Temporary(temporary) => {
                owner
                    .remove_temporary(temporary)
                    .context("remove a writer temporary")?;
                Ok(temporary.identity.len)
            }
            Self::OrphanIndex(address) => owner
                .remove_orphan_index(*address)
                .context("remove an orphan index"),
            Self::Segment(address) => Ok(owner
                .remove_finished_segment(*address)
                .context("remove a finished segment")?
                .total_bytes()),
        }
    }

    /// Orphan indexes have no scanned size to subtract from the tree counter.
    pub(super) const fn counted_in_tree(&self) -> bool {
        !matches!(self, Self::OrphanIndex(_))
    }

    pub(super) fn path(&self) -> String {
        match self {
            Self::Temporary(temporary) => {
                format!("{}/{}", temporary.address.day, temporary.file_name())
            }
            Self::OrphanIndex(address) => format!("{}/{}", address.day, address.idx_name()),
            Self::Segment(address) => format!("{}/{}", address.day, address.zms_name()),
        }
    }

    pub(super) fn log_removed(
        &self,
        reason: &'static str,
        freed: u64,
        current: u64,
        threshold: u64,
    ) {
        let (kind, address) = match self {
            Self::Temporary(temporary) => ("temporary", temporary.address),
            Self::OrphanIndex(address) => ("orphan_index", *address),
            Self::Segment(address) => ("segment", *address),
        };
        log_event(
            LogLevel::Info,
            "rotation_delete",
            &[
                field("kind", kind),
                field("path", self.path()),
                field("freed_bytes", freed),
                field("reason", reason),
                field("current_bytes", current),
                field("threshold_bytes", threshold),
                field("segment_id", address.id.get()),
            ],
        );
    }
}
