//! Bounded accounting for the metadata one scan is allowed to retain.

use std::fs::File;
use std::io;
use std::sync::Arc;

use kronika_format::{Catalog, Entry};
use kronika_layout::{FileIdentity, LayoutError, LimitKind};

use crate::catalog_summary::CatalogSummary;
use crate::source::{ActivePart, FinalUnit, StoreError, StoreWarning};

use super::segment::stale_finished_zms;
use super::{ACTIVE_ARC_ALLOCATION_BYTES, ARC_ALLOCATION_OVERHEAD};

pub(super) fn advance_previous(
    previous: &[FinalUnit],
    previous_at: &mut usize,
    current: kronika_layout::SegmentAddress,
) {
    while previous
        .get(*previous_at)
        .is_some_and(|finished| finished.address < current)
    {
        *previous_at += 1;
    }
}

pub(super) fn ensure_retained_metadata(
    retained_without_warnings: usize,
    additional: usize,
    warning_capacity: usize,
    limit: usize,
) -> io::Result<()> {
    let admitted = retained_without_warnings
        .checked_add(additional)
        .and_then(|bytes| {
            warning_capacity
                .checked_mul(size_of::<StoreWarning>())
                .and_then(|warning_bytes| bytes.checked_add(warning_bytes))
        })
        .is_some_and(|bytes| bytes <= limit);
    if admitted {
        Ok(())
    } else {
        Err(metadata_limit_io(limit))
    }
}

pub(super) fn retained_metadata_with_warnings(
    retained_without_warnings: usize,
    warning_capacity: usize,
) -> Option<usize> {
    warning_capacity
        .checked_mul(size_of::<StoreWarning>())
        .and_then(|warning_bytes| retained_without_warnings.checked_add(warning_bytes))
}

pub(super) fn push_warning_bounded(
    warnings: &mut Vec<StoreWarning>,
    warning: StoreWarning,
    retained_without_warnings: usize,
    limit: usize,
) -> io::Result<()> {
    if warnings.len() == warnings.capacity() {
        let required_capacity = warnings
            .len()
            .checked_add(1)
            .ok_or_else(|| metadata_limit_io(limit))?;
        ensure_retained_metadata(retained_without_warnings, 0, required_capacity, limit)?;
        warnings
            .try_reserve_exact(1)
            .map_err(|_error| metadata_limit_io(limit))?;
        // An allocator may satisfy an exact reservation with a larger block.
        // Check the real retained capacity before making it authoritative.
        ensure_retained_metadata(retained_without_warnings, 0, warnings.capacity(), limit)?;
    }
    warnings.push(warning);
    Ok(())
}

pub(super) fn ensure_scan_metadata_budget(
    layout_metadata: usize,
    journal_metadata: usize,
    previous_segments: usize,
    current_segments: usize,
    new_summaries: usize,
    limit: usize,
) -> io::Result<()> {
    let Some(accounted) = accounted_scan_metadata_bytes(
        layout_metadata,
        journal_metadata,
        previous_segments,
        current_segments,
        new_summaries,
    ) else {
        return Err(metadata_limit_io(limit));
    };
    if accounted > limit {
        return Err(metadata_limit_io(limit));
    }
    Ok(())
}

pub(super) fn accounted_scan_metadata_bytes(
    layout_metadata: usize,
    journal_metadata: usize,
    previous_segments: usize,
    current_segments: usize,
    new_summaries: usize,
) -> Option<usize> {
    let previous_capacity = previous_segments.checked_mul(size_of::<FinalUnit>())?;
    let current_capacity = current_segments.checked_mul(size_of::<FinalUnit>())?;
    let collection_count = usize::from(previous_segments != 0).checked_add(1)?;
    let finished_collections = collection_count
        .checked_mul(size_of::<Vec<FinalUnit>>().checked_add(ARC_ALLOCATION_OVERHEAD)?)?;
    let unique_summaries = previous_segments.checked_add(new_summaries)?;
    let summary_allocations = unique_summaries.checked_mul(summary_allocation_bytes())?;
    layout_metadata
        .checked_add(journal_metadata)?
        .checked_add(previous_capacity)?
        .checked_add(current_capacity)?
        .checked_add(finished_collections)?
        .checked_add(summary_allocations)
}

pub(crate) const fn summary_allocation_bytes() -> usize {
    size_of::<CatalogSummary>() + ARC_ALLOCATION_OVERHEAD
}

pub(super) fn catalog_metadata_bytes(catalog: &Catalog) -> io::Result<usize> {
    catalog
        .entries
        .capacity()
        .checked_mul(size_of::<Entry>())
        .ok_or_else(metadata_size_overflow)
}

pub(super) fn ensure_active_part_budget(
    retained_metadata: usize,
    part_metadata: usize,
    transient_part_bytes: usize,
    limit: usize,
) -> io::Result<()> {
    let admitted = retained_metadata
        .checked_add(part_metadata)
        .and_then(|retained| retained.checked_add(transient_part_bytes))
        .is_some_and(|peak| peak <= limit);
    if admitted {
        Ok(())
    } else {
        Err(metadata_limit_io(limit))
    }
}

pub(super) fn active_metadata_bytes(
    active: &[ActivePart],
    active_capacity: usize,
) -> io::Result<usize> {
    let struct_bytes = active_capacity
        .checked_mul(size_of::<ActivePart>())
        .and_then(|bytes| bytes.checked_add(ACTIVE_ARC_ALLOCATION_BYTES))
        .ok_or_else(metadata_size_overflow)?;
    active.iter().try_fold(struct_bytes, |total, part| {
        let entries = part
            .catalog
            .entries
            .capacity()
            .checked_mul(size_of::<Entry>())
            .ok_or_else(metadata_size_overflow)?;
        total
            .checked_add(entries)
            .ok_or_else(metadata_size_overflow)
    })
}

#[expect(
    clippy::rc_buffer,
    reason = "capacity admission and copy-on-write apply to the retained Vec allocation"
)]
pub(super) fn reserve_active_slots(
    active: &mut Arc<Vec<ActivePart>>,
    additional: usize,
    retained_metadata: usize,
    transient_metadata: usize,
    previous_active_metadata: usize,
    limit: usize,
) -> io::Result<usize> {
    let final_len = active
        .len()
        .checked_add(additional)
        .ok_or_else(metadata_size_overflow)?;
    if additional != 0 {
        let clone_peak = previous_active_metadata
            .checked_add(retained_metadata)
            .and_then(|peak| peak.checked_add(transient_metadata))
            .is_some_and(|peak| peak <= limit);
        if !clone_peak {
            return Err(metadata_limit_io(limit));
        }
        Arc::make_mut(active);
    }
    if final_len > active.capacity() {
        // `Vec` reallocation can retain the old allocation until the replacement
        // has succeeded, so admit both allocations before asking the allocator.
        let replacement_allocation = final_len
            .checked_mul(size_of::<ActivePart>())
            .ok_or_else(metadata_size_overflow)?;
        let admitted = previous_active_metadata
            .checked_add(retained_metadata)
            .and_then(|peak| peak.checked_add(transient_metadata))
            .and_then(|peak| peak.checked_add(replacement_allocation))
            .is_some_and(|peak| peak <= limit);
        if !admitted {
            return Err(metadata_limit_io(limit));
        }
        Arc::make_mut(active)
            .try_reserve_exact(additional)
            .map_err(|_error| metadata_limit_io(limit))?;
    }
    let retained_after = active_metadata_bytes(active, active.capacity())?;
    if previous_active_metadata
        .checked_add(retained_after)
        .and_then(|peak| peak.checked_add(transient_metadata))
        .is_none_or(|peak| peak > limit)
    {
        return Err(metadata_limit_io(limit));
    }
    Ok(retained_after)
}

pub(super) fn metadata_size_overflow() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "active journal metadata size overflow",
    )
}

pub(super) fn metadata_limit_io(limit: usize) -> io::Error {
    layout_io(LayoutError::TraversalLimitExceeded {
        kind: LimitKind::MetadataBytes,
        limit,
    })
}

pub(super) const fn metadata_limit_store(limit: usize) -> StoreError {
    StoreError::Layout(LayoutError::TraversalLimitExceeded {
        kind: LimitKind::MetadataBytes,
        limit,
    })
}

pub(super) fn require_file_identity(
    file: &File,
    expected: FileIdentity,
    address: kronika_layout::SegmentAddress,
    phase: &str,
) -> io::Result<()> {
    let actual = FileIdentity::from_file(file)?;
    if actual == expected {
        return Ok(());
    }
    Err(stale_finished_zms(address, phase))
}

pub(super) fn layout_io(error: LayoutError) -> io::Error {
    match error {
        LayoutError::Io(source) => source,
        structural => io::Error::new(io::ErrorKind::InvalidData, structural),
    }
}

pub(super) fn store_io(error: StoreError) -> io::Error {
    let kind = match error {
        StoreError::Io(ref source) => source.kind(),
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, error)
}
