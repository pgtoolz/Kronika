//! Finished-only query adapter over immutable storage sources.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::ops::Bound;

use kronika_reader::{FinishedReader, Segment, SegmentKind, SegmentSection};
use kronika_store::{
    ImmutableSegmentSource, ResourceWarning, ResourceWarningSubject, SegmentResource,
};

use crate::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    OpaqueCapture, PredecessorSelection, QueryDataset, QueryError, SegmentBounds, SegmentSelection,
};

/// A finished-only dataset backed by one immutable storage source.
#[derive(Debug)]
pub struct FinishedDataset<S> {
    reader: FinishedReader<S>,
}

impl<S> FinishedDataset<S> {
    /// Bind query execution to an immutable source.
    #[must_use]
    pub const fn new(source: S) -> Self {
        Self {
            reader: FinishedReader::new(source),
        }
    }
}

impl<S> FinishedDataset<S>
where
    S: ImmutableSegmentSource + Debug + Send + Sync + 'static,
{
    fn descriptor(
        &self,
        resource: &SegmentResource<S::Resource>,
    ) -> Result<DatasetSegment, QueryError> {
        let segment = self.reader.open_segment(resource)?;
        let sections = segment
            .sections()
            .map(|(type_id, section)| SegmentSection {
                type_id,
                rows: section.rows,
                bytes: section.bytes,
            })
            .collect::<Vec<_>>()
            .into();
        Ok(DatasetSegment::new(
            OpaqueCapture::new(resource.clone()),
            segment.id(),
            SegmentKind::Finished,
            segment.min_ts(),
            segment.max_ts(),
            None,
            sections,
        ))
    }

    fn captured(segment: &DatasetSegment) -> Result<&SegmentResource<S::Resource>, QueryError> {
        segment.capture().downcast_ref().ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "segment capture belongs to another immutable dataset",
            )))
        })
    }
}

#[derive(Debug)]
struct FinishedCatalog<'a, S>
where
    S: ImmutableSegmentSource,
{
    dataset: &'a FinishedDataset<S>,
    resources: Vec<SegmentResource<S::Resource>>,
    warnings: Vec<DatasetWarning>,
    ranges: Vec<(i64, i64)>,
}

impl<S> CapturedCatalog for FinishedCatalog<'_, S>
where
    S: ImmutableSegmentSource + Debug + Send + Sync + 'static,
{
    fn ranges(&self) -> &[(i64, i64)] {
        &self.ranges
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        let mut selected = BTreeMap::new();
        for resource in self.resources.iter().filter(|resource| {
            overlaps(
                selection.bounds,
                resource.summary().min_ts,
                resource.summary().max_ts,
            )
        }) {
            selected.insert(
                resource.identity().segment_id().get(),
                self.dataset.descriptor(resource)?,
            );
        }

        match &selection.predecessor {
            PredecessorSelection::None => {}
            PredecessorSelection::Closest => {
                if let Some(resource) = closest_predecessor(&self.resources, selection.bounds) {
                    selected
                        .entry(resource.identity().segment_id().get())
                        .or_insert(self.dataset.descriptor(resource)?);
                }
            }
            PredecessorSelection::ForLayouts(type_ids) => {
                let mut remaining = type_ids.iter().copied().collect::<BTreeSet<_>>();
                let mut candidates = self
                    .resources
                    .iter()
                    .filter(|resource| before_start(selection.bounds, resource.summary().max_ts))
                    .collect::<Vec<_>>();
                candidates.sort_unstable_by_key(|resource| {
                    std::cmp::Reverse((
                        resource.summary().max_ts,
                        resource.identity().segment_id().get(),
                    ))
                });
                for resource in candidates {
                    let requested = remaining.iter().copied().collect::<Vec<_>>();
                    if requested.is_empty() {
                        break;
                    }
                    if !resource.summary().may_contain_any_nonempty_type(&requested) {
                        continue;
                    }
                    let descriptor = self.dataset.descriptor(resource)?;
                    let matched = descriptor
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
                    selected.insert(resource.identity().segment_id().get(), descriptor);
                }
            }
        }

        Ok(DatasetListing {
            segments: selected.into_values().collect(),
            warnings: self.warnings.clone(),
        })
    }
}

impl<S> QueryDataset for FinishedDataset<S>
where
    S: ImmutableSegmentSource + Debug + Send + Sync + 'static,
{
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        let listing = self.reader.resources()?;
        let ranges = listing
            .resources
            .iter()
            .map(|resource| (resource.summary().min_ts, resource.summary().max_ts))
            .collect();
        Ok(Box::new(FinishedCatalog {
            dataset: self,
            resources: listing.resources,
            warnings: warnings(listing.warnings),
            ranges,
        }))
    }

    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError> {
        let listing = self.reader.resources()?;
        let segments = listing
            .resources
            .iter()
            .filter(|resource| resource.identity().segment_id().get() == id)
            .map(|resource| self.descriptor(resource))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DatasetListing {
            segments,
            warnings: warnings(listing.warnings),
        })
    }

    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError> {
        self.reader
            .open_segment(Self::captured(segment)?)
            .map_err(QueryError::from)
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        Err(QueryError::BadCursor)
    }
}

fn warnings(warnings: Vec<ResourceWarning>) -> Vec<DatasetWarning> {
    warnings
        .into_iter()
        .map(|warning| {
            let subject = match warning.subject() {
                ResourceWarningSubject::FinishedSegment(identity) => {
                    DatasetWarningSubject::Segment(identity.segment_id().get())
                }
                _ => DatasetWarningSubject::Other,
            };
            DatasetWarning {
                subject,
                code: warning.code(),
            }
        })
        .collect()
}

fn closest_predecessor<R>(
    resources: &[SegmentResource<R>],
    bounds: SegmentBounds,
) -> Option<&SegmentResource<R>> {
    resources
        .iter()
        .filter(|resource| before_start(bounds, resource.summary().max_ts))
        .max_by_key(|resource| {
            (
                resource.summary().max_ts,
                resource.identity().segment_id().get(),
            )
        })
}

const fn overlaps(bounds: SegmentBounds, min_ts: i64, max_ts: i64) -> bool {
    !before_start(bounds, max_ts) && !after_end(bounds, min_ts)
}

const fn before_start(bounds: SegmentBounds, max_ts: i64) -> bool {
    match bounds.start {
        Bound::Included(start) => max_ts < start,
        Bound::Excluded(start) => max_ts <= start,
        Bound::Unbounded => false,
    }
}

const fn after_end(bounds: SegmentBounds, min_ts: i64) -> bool {
    match bounds.end {
        Bound::Included(end) => min_ts > end,
        Bound::Excluded(end) => min_ts >= end,
        Bound::Unbounded => false,
    }
}
