//! Reading a `Kronika` data directory back into rows.
//!
//! [`Reader::segments`] lists finished segments and the captured current
//! `active.wal` segment whose timestamps fall in a range;
//! [`Reader::open_segment`] opens either kind through the same row API.
//!
//! Nothing is cached between calls. A `Reader` that no one is asking holds an
//! open directory descriptor and nothing else.

mod dictionary;
mod error;
mod segment;

#[cfg(feature = "posix")]
use std::cmp::Reverse;
#[cfg(feature = "posix")]
use std::collections::BTreeSet;
#[cfg(feature = "posix")]
use std::ops::{Bound, RangeBounds};
#[cfg(feature = "posix")]
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "posix")]
use kronika_format::Catalog;
#[cfg(feature = "posix")]
use kronika_store::{ActiveSnapshot, FinalUnit, LocalDir, read_catalog};
use kronika_store::{
    ImmutableSegmentSource, ResourceCatalog, ResourceError, ResourceListing, SegmentResource,
    read_resource_catalog,
};

pub use dictionary::{Dictionary, OwnedDictionaryValue};
pub use error::ReaderError;
pub use kronika_format::{BlobEntry, Resolved, StrId};
pub use kronika_registry::{Cell, RecordBatch, Row};
#[cfg(feature = "posix")]
pub use kronika_store::{StoreObject, StoreWarning, StoreWarningReason};
pub use segment::{Section, Segment};

/// Product reader for immutable segments from one storage source.
///
/// Catalog discovery stays separate from opening positional bytes. The source
/// decides how an object is prepared; decoding remains synchronous.
#[derive(Debug)]
pub struct FinishedReader<S> {
    source: S,
}

impl<S> FinishedReader<S> {
    /// Bind a product reader to one immutable source.
    #[must_use]
    pub const fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: ResourceCatalog> FinishedReader<S> {
    /// Discover immutable identities and compact catalogs.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the bounded catalog pass cannot complete.
    pub fn resources(&self) -> Result<ResourceListing<S::Resource>, ReaderError> {
        let mut listing = self.source.resources()?;
        listing
            .resources
            .sort_unstable_by_key(SegmentResource::identity);
        if let Some(identity) = listing
            .resources
            .windows(2)
            .find(|pair| pair[0].identity() == pair[1].identity())
            .map(|pair| pair[0].identity())
        {
            return Err(ResourceError::DuplicateIdentity(identity).into());
        }
        Ok(listing)
    }
}

impl<S: ImmutableSegmentSource> FinishedReader<S> {
    /// Open one discovered resource through the production row decoder.
    ///
    /// # Errors
    ///
    /// Returns an error for a foreign or changed resource, an unreadable
    /// object, or an invalid full catalog.
    pub fn open_segment(
        &self,
        resource: &SegmentResource<S::Resource>,
    ) -> Result<Segment, ReaderError> {
        let bytes = self.source.open_resource(resource)?;
        let catalog = read_resource_catalog(&bytes);
        self.source.validate_opened(resource, &bytes)?;
        let catalog = Arc::new(catalog?);
        Ok(Segment::open_finished(
            bytes,
            catalog,
            resource.identity().segment_id().get(),
            resource.captured_bytes(),
            resource.summary(),
            format!("segment:{}", resource.identity().segment_id().get()),
        ))
    }
}

/// Whether a listed segment is immutable or the captured journal prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    /// An immutable `.zms` file.
    Finished,
    /// The committed prefix of `active.wal` captured by the listing.
    Active,
}

/// One physical section actually present in a listed segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentSection {
    /// Exact physical registry layout.
    pub type_id: u32,
    /// Rows recorded by the segment catalog.
    pub rows: u64,
    /// Compressed section body bytes.
    pub bytes: u64,
}

#[cfg(feature = "posix")]
#[derive(Debug, Clone)]
enum SegmentSource {
    Finished(FinalUnit),
    Active(ActiveSnapshot),
}

/// One segment captured by a directory listing.
///
/// The underlying source is deliberately opaque so finished `.zms` files and
/// the current `active.wal` prefix are opened through the same API.
#[cfg(feature = "posix")]
#[derive(Debug, Clone)]
pub struct SegmentRef {
    source: SegmentSource,
    provenance: Arc<()>,
    segment_id: i64,
    min_ts: i64,
    max_ts: i64,
    captured_bytes: u64,
    sections: Arc<[SegmentSection]>,
}

#[cfg(feature = "posix")]
impl SegmentRef {
    /// Stable segment id: unix microseconds of its first appended window.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.segment_id
    }

    /// Whether this reference names an immutable file or captured live data.
    #[must_use]
    pub const fn kind(&self) -> SegmentKind {
        match self.source {
            SegmentSource::Finished(_) => SegmentKind::Finished,
            SegmentSource::Active(_) => SegmentKind::Active,
        }
    }

    /// Exact committed journal-prefix position for an active reference.
    ///
    /// Finished segments have no journal position and return `None`.
    #[must_use]
    pub const fn active_position(&self) -> Option<u64> {
        match self.source {
            SegmentSource::Finished(_) => None,
            SegmentSource::Active(_) => Some(self.captured_bytes),
        }
    }

    /// Earliest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.min_ts
    }

    /// Latest timestamp the segment carries, unix microseconds.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.max_ts
    }

    /// Physical sections actually present, in numeric layout order.
    #[must_use]
    pub fn sections(&self) -> &[SegmentSection] {
        &self.sections
    }

    /// Share the compact section catalog without copying its entries.
    #[must_use]
    pub fn shared_sections(&self) -> Arc<[SegmentSection]> {
        Arc::clone(&self.sections)
    }

    /// Pin an active reference to an earlier committed cursor position.
    ///
    /// Finished references and positions that are not complete frame
    /// boundaries are rejected.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input or store framing error for an unusable
    /// position.
    pub fn at_active_position(&self, position: u64) -> Result<Self, ReaderError> {
        let SegmentSource::Active(snapshot) = &self.source else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a finished segment has no active position",
            )
            .into());
        };
        let snapshot = snapshot.at_position(position)?;
        let (min_ts, max_ts) = active_bounds(snapshot.parts()).unwrap_or((0, 0));
        let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
        Ok(Self {
            source: SegmentSource::Active(snapshot),
            provenance: Arc::clone(&self.provenance),
            segment_id: self.segment_id,
            min_ts,
            max_ts,
            captured_bytes: position,
            sections,
        })
    }
}

/// What one directory scan found.
#[cfg(feature = "posix")]
#[derive(Debug)]
pub struct Listing {
    /// Finished and current segments overlapping the requested range, oldest
    /// first.
    pub segments: Vec<SegmentRef>,
    /// Files the scan set aside, and why. Passing over a damaged segment
    /// without a word would report a quiet day instead of a broken one.
    pub warnings: Vec<StoreWarning>,
}

/// One catalog-only store scan whose full section catalogs remain unopened.
///
/// A caller can inspect all recorded time ranges, choose a window, and then
/// materialize references only for segments that overlap that window.
#[cfg(feature = "posix")]
#[derive(Debug, Clone)]
pub struct CatalogDiscovery<'a> {
    reader: &'a Reader,
    scan: kronika_store::LocalScan,
}

#[cfg(feature = "posix")]
#[derive(Clone, Copy)]
enum ListingMode {
    Catalog,
    CatalogWithPredecessor,
    Validated,
}

#[cfg(feature = "posix")]
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

    fn list_segments<R: RangeBounds<i64>>(
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

/// An open data directory.
#[cfg(feature = "posix")]
#[derive(Debug)]
pub struct Reader {
    dir: LocalDir,
    root: PathBuf,
    provenance: Arc<()>,
}

#[cfg(feature = "posix")]
impl Reader {
    /// Open `root` as a data directory.
    ///
    /// Only the directory descriptor is opened here; nothing is read until
    /// [`segments`](Self::segments).
    ///
    /// # Errors
    ///
    /// Returns an I/O error when `root` is not a directory or cannot be
    /// accessed.
    pub fn open(root: &Path) -> Result<Self, ReaderError> {
        Ok(Self {
            dir: LocalDir::open(root)?,
            root: root.to_path_buf(),
            provenance: Arc::new(()),
        })
    }

    /// Native data-directory path retained for sibling derived resources.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// List finished segments and the captured current segment whose
    /// timestamps overlap `range`.
    ///
    /// The range is in unix microseconds, and `..` asks for everything. A
    /// segment covers an interval, so it is listed when any part of that
    /// interval falls inside the range.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be walked.
    pub fn segments<R: RangeBounds<i64>>(&self, range: R) -> Result<Listing, ReaderError> {
        self.list_segments(range, ListingMode::Validated)
    }

    /// Scan compact catalog summaries before choosing which full catalogs to
    /// open.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory cannot be walked or a compact
    /// catalog summary cannot be read safely.
    pub fn catalog_discovery(&self) -> Result<CatalogDiscovery<'_>, ReaderError> {
        Ok(CatalogDiscovery {
            reader: self,
            scan: self.dir.scan_catalogs()?,
        })
    }

    /// Find one segment by its stable id without opening unrelated catalogs.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory or the selected segment catalog
    /// cannot be read safely.
    pub fn catalog_segment(&self, id: i64) -> Result<Listing, ReaderError> {
        let scan = self.dir.scan_catalogs()?;
        let mut segments = Vec::with_capacity(1);
        if let Some(unit) = scan
            .finished
            .iter()
            .find(|unit| unit.address.id.get() == id)
        {
            let file = self.dir.open_finished(unit)?;
            let catalog = read_catalog(&file)?;
            self.dir.validate_finished_file(&file, unit)?;
            segments.push(SegmentRef {
                source: SegmentSource::Finished(unit.clone()),
                provenance: Arc::clone(&self.provenance),
                segment_id: unit.address.id.get(),
                min_ts: unit.summary.min_ts,
                max_ts: unit.summary.max_ts,
                captured_bytes: unit.identity.len,
                sections: sections_of(std::iter::once(&catalog)).into(),
            });
        } else if scan
            .active
            .first()
            .is_some_and(|part| part.segment_id.get() == id)
            && let Some(snapshot) = self.dir.open_active_snapshot(&scan)?
        {
            let (min_ts, max_ts) = active_bounds(snapshot.parts()).unwrap_or((0, 0));
            let sections = sections_of(snapshot.parts().iter().map(|part| &part.catalog)).into();
            segments.push(SegmentRef {
                segment_id: snapshot.segment_id().get(),
                source: SegmentSource::Active(snapshot),
                provenance: Arc::clone(&self.provenance),
                min_ts,
                max_ts,
                captured_bytes: scan.valid_len,
                sections,
            });
        }
        Ok(Listing {
            segments,
            warnings: scan.warnings,
        })
    }

    fn list_segments<R: RangeBounds<i64>>(
        &self,
        range: R,
        mode: ListingMode,
    ) -> Result<Listing, ReaderError> {
        self.catalog_discovery()?.list_segments(range, mode)
    }

    /// Open one of the segments a listing returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is gone, changed under the listing, or
    /// its catalog is rejected. A reference returned by another reader is
    /// rejected as invalid input.
    pub fn open_segment(&self, unit: &SegmentRef) -> Result<Segment, ReaderError> {
        if !Arc::ptr_eq(&self.provenance, &unit.provenance) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "segment reference belongs to another reader",
            )
            .into());
        }
        let source_label = match &unit.source {
            SegmentSource::Finished(finished) => {
                let day = finished.address.day;
                self.root
                    .join(day.year_component())
                    .join(day.month_component())
                    .join(day.day_component())
                    .join(finished.address.zms_name())
            }
            SegmentSource::Active(_) => self.root.join("active.wal"),
        }
        .display()
        .to_string();
        Segment::open(&self.dir, unit, source_label)
    }
}

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
#[cfg(feature = "posix")]
fn sections_of<'a>(catalogs: impl IntoIterator<Item = &'a Catalog>) -> Vec<SegmentSection> {
    let mut sections = std::collections::BTreeMap::<u32, SegmentSection>::new();
    for catalog in catalogs {
        for entry in &catalog.entries {
            let section = sections.entry(entry.type_id).or_insert(SegmentSection {
                type_id: entry.type_id,
                rows: 0,
                bytes: 0,
            });
            section.rows = section.rows.saturating_add(u64::from(entry.rows));
            section.bytes = section.bytes.saturating_add(entry.len);
        }
    }
    sections.into_values().collect()
}

#[cfg(feature = "posix")]
fn active_bounds(parts: &[kronika_store::ActivePart]) -> Option<(i64, i64)> {
    if parts.is_empty() {
        return None;
    }
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    for part in parts {
        min_ts = min_ts.min(part.catalog.min_ts);
        max_ts = max_ts.max(part.catalog.max_ts);
    }
    if min_ts > max_ts {
        Some((0, 0))
    } else {
        Some((min_ts, max_ts))
    }
}

/// Whether a segment covering `[min_ts, max_ts]` has anything inside `range`.
///
/// Timestamps are whole microseconds, so an excluded bound moves one
/// microsecond inwards and both ends become inclusive. An empty range then
/// yields no instants at all and matches nothing.
#[cfg(feature = "posix")]
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

#[cfg(feature = "posix")]
fn before_start<R: RangeBounds<i64>>(range: &R, max_ts: i64) -> bool {
    match range.start_bound() {
        Bound::Unbounded => false,
        Bound::Included(start) => max_ts < *start,
        Bound::Excluded(start) => max_ts <= *start,
    }
}

#[cfg(feature = "posix")]
fn owned_bounds<R: RangeBounds<i64>>(range: &R) -> (Bound<i64>, Bound<i64>) {
    (range.start_bound().cloned(), range.end_bound().cloned())
}

#[cfg(all(test, feature = "posix"))]
mod tests;
