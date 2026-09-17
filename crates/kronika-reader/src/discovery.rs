//! Range selection and canonical predecessor discovery from one captured scan.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::ops::{Bound, RangeBounds};
use std::sync::Arc;

use kronika_store::read_catalog;

use crate::{Listing, Reader, ReaderError, SegmentRef, SegmentSource, active_bounds, sections_of};

/// One catalog-only store scan whose full section catalogs remain unopened.
///
/// A caller can inspect all recorded time ranges, choose a window, and then
/// materialize references only for segments that overlap that window.
#[derive(Debug, Clone)]
pub struct CatalogDiscovery<'a> {
    pub(super) reader: &'a Reader,
    pub(super) scan: kronika_store::LocalScan,
}

#[derive(Clone, Copy)]
pub(super) enum ListingMode {
    Catalog,
    CatalogWithPredecessor,
    Validated,
}

impl CatalogDiscovery<'_> {
    /// Time bounds of every canonical segment found by the scan.
    pub fn ranges(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        let active_id = self.scan.active.first().map(|part| part.segment_id.get());
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            self.scan
                .finished
                .iter()
                .any(|unit| unit.address.id.get() == active_id)
        });
        let active = (!finished_is_canonical)
            .then(|| active_bounds(&self.scan.active))
            .flatten();
        self.scan
            .finished
            .iter()
            .map(|unit| (unit.summary.min_ts, unit.summary.max_ts))
            .chain(active)
    }

    /// Open section catalogs only for segments overlapping `range`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when a selected segment changed or its catalog
    /// cannot be read safely.
    pub fn segments<R: RangeBounds<i64>>(self, range: R) -> Result<Listing, ReaderError> {
        self.list_segments(range, ListingMode::Catalog)
    }

    /// Open section catalogs in `range` and the closest canonical predecessor.
    ///
    /// This uses the same captured directory scan as [`Self::ranges`].
    ///
    /// # Errors
    ///
    /// Returns an error when the captured segment catalog cannot be read.
    pub fn segments_with_predecessor<R: RangeBounds<i64>>(
        self,
        range: R,
    ) -> Result<Listing, ReaderError> {
        self.list_segments(range, ListingMode::CatalogWithPredecessor)
    }

    /// Open section catalogs in `range` and the closest predecessor carrying
    /// rows for each requested physical layout.
    ///
    /// Compact finished-segment summaries reject sectionless candidates before
    /// their full catalogs are opened. Positive summary matches are confirmed
    /// against the catalog, so a Bloom-filter collision cannot hide an older
    /// compatible predecessor. The active segment is considered from the same
    /// captured directory scan.
    ///
    /// # Errors
    ///
    /// Returns an error when a selected segment catalog cannot be read safely.
    pub fn segments_with_predecessors_for<R: RangeBounds<i64>>(
        self,
        range: R,
        type_ids: &[u32],
    ) -> Result<Listing, ReaderError> {
        let bounds = owned_bounds(&range);
        let mut listing = self.clone().list_segments(bounds, ListingMode::Catalog)?;
        let mut remaining = type_ids.iter().copied().collect::<BTreeSet<_>>();
        if remaining.is_empty() {
            return Ok(listing);
        }

        let finished = Arc::clone(&self.scan.finished);
        let active_id = self.scan.active.first().map(|part| part.segment_id.get());
        let active_time_bounds = active_bounds(&self.scan.active);
        let finished_exists = active_id.is_some_and(|active_id| {
            finished
                .iter()
                .any(|unit| unit.address.id.get() == active_id)
        });
        let canonical_active = (!finished_exists)
            .then_some(active_id.zip(active_time_bounds))
            .flatten();
        let mut candidates = finished
            .iter()
            .enumerate()
            .filter(|(_index, unit)| before_start(&range, unit.summary.max_ts))
            .map(|(index, unit)| (unit.summary.max_ts, unit.address.id.get(), Some(index)))
            .chain(
                canonical_active
                    .filter(|(_id, (_min_ts, max_ts))| before_start(&range, *max_ts))
                    .map(|(id, (_min_ts, max_ts))| (max_ts, id, None)),
            )
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(max_ts, id, _index)| Reverse((*max_ts, *id)));

        for (_max_ts, _id, finished_index) in candidates {
            let requested = remaining.iter().copied().collect::<Vec<_>>();
            let segment = if let Some(index) = finished_index {
                let unit = &finished[index];
                if !unit.summary.may_contain_any_nonempty_type(&requested) {
                    continue;
                }
                let file = self.reader.dir.open_finished(unit)?;
                let catalog = read_catalog(&file)?;
                self.reader.dir.validate_finished_file(&file, unit)?;
                let sections = sections_of(std::iter::once(&catalog)).into();
                SegmentRef {
                    source: SegmentSource::Finished(unit.clone()),
                    provenance: Arc::clone(&self.reader.provenance),
                    segment_id: unit.address.id.get(),
                    min_ts: unit.summary.min_ts,
                    max_ts: unit.summary.max_ts,
                    captured_bytes: unit.identity.len,
                    sections,
                }
            } else {
                let Some(snapshot) = self.reader.dir.open_active_snapshot(&self.scan)? else {
                    continue;
                };
                let Some((min_ts, max_ts)) = active_time_bounds else {
                    continue;
                };
                let sections =
                    sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
                SegmentRef {
                    segment_id: snapshot.segment_id().get(),
                    source: SegmentSource::Active(snapshot),
                    provenance: Arc::clone(&self.reader.provenance),
                    min_ts,
                    max_ts,
                    captured_bytes: self.scan.valid_len,
                    sections,
                }
            };
            let matched = segment
                .sections()
                .iter()
                .filter(|section| section.rows > 0 && remaining.contains(&section.type_id))
                .map(|section| section.type_id)
                .collect::<Vec<_>>();
            if matched.is_empty() {
                continue;
            }
            for type_id in matched {
                remaining.remove(&type_id);
            }
            listing.segments.push(segment);
            if remaining.is_empty() {
                break;
            }
        }
        listing.segments.sort_unstable_by_key(SegmentRef::id);
        Ok(listing)
    }

    pub(super) fn list_segments<R: RangeBounds<i64>>(
        mut self,
        range: R,
        mode: ListingMode,
    ) -> Result<Listing, ReaderError> {
        let mut segments = Vec::new();
        let finished = Arc::clone(&self.scan.finished);
        let active_id = self.scan.active.first().map(|part| part.segment_id.get());
        let active_time_bounds = active_bounds(&self.scan.active);
        let finished_exists = active_id.is_some_and(|active_id| {
            finished
                .iter()
                .any(|unit| unit.address.id.get() == active_id)
        });
        let canonical_active = (matches!(mode, ListingMode::Validated) || !finished_exists)
            .then_some(active_id.zip(active_time_bounds))
            .flatten();
        let predecessor = matches!(mode, ListingMode::CatalogWithPredecessor)
            .then(|| {
                finished
                    .iter()
                    .filter(|unit| before_start(&range, unit.summary.max_ts))
                    .map(|unit| (unit.summary.max_ts, unit.address.id.get()))
                    .chain(
                        canonical_active
                            .filter(|(_id, (_min_ts, max_ts))| before_start(&range, *max_ts))
                            .map(|(id, (_min_ts, max_ts))| (max_ts, id)),
                    )
                    .max()
                    .map(|(_max_ts, id)| id)
            })
            .flatten();
        for unit in finished.iter().filter(|unit| {
            overlaps(&range, unit.summary.min_ts, unit.summary.max_ts)
                || predecessor == Some(unit.address.id.get())
        }) {
            if matches!(mode, ListingMode::Validated)
                && !self.reader.dir.validate_finished(&mut self.scan, unit)?
            {
                continue;
            }
            let file = self.reader.dir.open_finished(unit)?;
            let catalog = read_catalog(&file)?;
            self.reader.dir.validate_finished_file(&file, unit)?;
            let sections = sections_of(std::iter::once(&catalog)).into();
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.reader.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
                captured_bytes: unit.identity.len,
                sections,
            });
        }
        let finished_is_canonical = active_id.is_some_and(|active_id| {
            segments
                .iter()
                .any(|segment| segment.segment_id == active_id)
        });
        let active = if finished_is_canonical {
            None
        } else if let Some((_id, (min_ts, max_ts))) =
            canonical_active.filter(|(id, (min_ts, max_ts))| {
                overlaps(&range, *min_ts, *max_ts) || predecessor == Some(*id)
            })
        {
            self.reader
                .dir
                .open_active_snapshot(&self.scan)?
                .map(|snapshot| (snapshot, min_ts, max_ts))
        } else {
            None
        };
        if let Some((snapshot, min_ts, max_ts)) = active {
            let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
            segments.push(SegmentRef {
                segment_id: snapshot.segment_id().get(),
                source: SegmentSource::Active(snapshot),
                provenance: Arc::clone(&self.reader.provenance),
                min_ts,
                max_ts,
                captured_bytes: self.scan.valid_len,
                sections,
            });
        }
        segments.sort_by_key(|segment| segment.segment_id);
        Ok(Listing {
            segments,
            warnings: self.scan.warnings,
        })
    }
}

/// Whether a segment covering `[min_ts, max_ts]` has anything inside `range`.
///
/// Timestamps are whole microseconds, so an excluded bound moves one
/// microsecond inwards and both ends become inclusive. An empty range then
/// yields no instants at all and matches nothing.
fn overlaps<R: RangeBounds<i64>>(range: &R, min_ts: i64, max_ts: i64) -> bool {
    let start = match range.start_bound() {
        Bound::Unbounded => i64::MIN,
        Bound::Included(start) => *start,
        Bound::Excluded(start) => {
            let Some(start) = start.checked_add(1) else {
                return false;
            };
            start
        }
    };
    let end = match range.end_bound() {
        Bound::Unbounded => i64::MAX,
        Bound::Included(end) => *end,
        Bound::Excluded(end) => {
            let Some(end) = end.checked_sub(1) else {
                return false;
            };
            end
        }
    };
    min_ts.max(start) <= max_ts.min(end)
}

fn before_start<R: RangeBounds<i64>>(range: &R, max_ts: i64) -> bool {
    match range.start_bound() {
        Bound::Unbounded => false,
        Bound::Included(start) => max_ts < *start,
        Bound::Excluded(start) => max_ts <= *start,
    }
}

fn owned_bounds<R: RangeBounds<i64>>(range: &R) -> (Bound<i64>, Bound<i64>) {
    (range.start_bound().cloned(), range.end_bound().cloned())
}

#[cfg(test)]
#[path = "tests/ranges.rs"]
mod tests;
