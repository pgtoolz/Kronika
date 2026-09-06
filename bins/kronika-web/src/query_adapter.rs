//! Native captured-data adapter for shared query execution.

use std::path::Path;
use std::time::Instant;

use kronika_query::{
    CapturedCatalog, DatasetListing, DatasetSegment, DatasetWarning, DatasetWarningSubject,
    IndexProvider, IndexResource, OpaqueCapture, PredecessorSelection, QueryDataset, QueryError,
    SegmentSelection,
};
use kronika_reader::{CatalogDiscovery, Listing, Reader, Segment, SegmentRef, StoreObject};

/// One native reader retained for every opaque segment capture it produced.
#[derive(Debug)]
pub(crate) struct NativeDataset {
    reader: Reader,
    started: Instant,
}

impl NativeDataset {
    pub(crate) fn from_root(root: &Path) -> Result<Self, QueryError> {
        let started = Instant::now();
        Ok(Self {
            reader: Reader::open(root)?,
            started,
        })
    }

    fn descriptor(segment: SegmentRef) -> DatasetSegment {
        let sections = segment.shared_sections();
        let id = segment.id();
        let kind = segment.kind();
        let min_ts = segment.min_ts();
        let max_ts = segment.max_ts();
        let active_position = segment.active_position();
        DatasetSegment::new(
            OpaqueCapture::new(segment),
            id,
            kind,
            min_ts,
            max_ts,
            active_position,
            sections,
        )
    }

    fn listing(listing: Listing) -> DatasetListing {
        DatasetListing {
            segments: listing.segments.into_iter().map(Self::descriptor).collect(),
            warnings: listing.warnings.into_iter().map(warning).collect(),
        }
    }

    fn captured(segment: &DatasetSegment) -> Result<&SegmentRef, QueryError> {
        segment.capture().downcast_ref().ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "segment capture belongs to another data adapter",
            )))
        })
    }
}

#[derive(Debug)]
struct NativeCatalog<'a> {
    owner: &'a NativeDataset,
    discovery: CatalogDiscovery<'a>,
    ranges: Vec<(i64, i64)>,
}

impl CapturedCatalog for NativeCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        &self.ranges
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        let bounds = (selection.bounds.start, selection.bounds.end);
        let listing = match selection.predecessor {
            PredecessorSelection::None => self.discovery.clone().segments(bounds),
            PredecessorSelection::Closest => {
                self.discovery.clone().segments_with_predecessor(bounds)
            }
            PredecessorSelection::ForLayouts(type_ids) => self
                .discovery
                .clone()
                .segments_with_predecessors_for(bounds, &type_ids),
        }?;
        log_catalog_open(listing.segments.len(), &listing, self.owner.started);
        Ok(NativeDataset::listing(listing))
    }
}

impl QueryDataset for NativeDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        let discovery = self.reader.catalog_discovery()?;
        let ranges = discovery.ranges().collect();
        Ok(Box::new(NativeCatalog {
            owner: self,
            discovery,
            ranges,
        }))
    }

    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError> {
        let listing = self.reader.catalog_segment(id)?;
        log_warnings(&listing);
        if let Some(segment) = listing.segments.first() {
            eprintln!(
                "kronika-web: segment_open id={} kind={} sections={} elapsed_us={}",
                segment.id(),
                match segment.kind() {
                    kronika_reader::SegmentKind::Finished => "finished",
                    kronika_reader::SegmentKind::Active => "active",
                },
                segment.sections().len(),
                self.started.elapsed().as_micros(),
            );
        }
        Ok(Self::listing(listing))
    }

    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError> {
        self.reader
            .open_segment(Self::captured(segment)?)
            .map_err(QueryError::from)
    }

    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        let pinned = Self::captured(segment)?
            .at_active_position(position)
            .map_err(QueryError::from)?;
        Ok(Self::descriptor(pinned))
    }
}

impl IndexProvider for NativeDataset {
    fn load(
        &self,
        segment: &DatasetSegment,
        logical_name: &str,
        keys: &[kronika_index::SeriesKey],
    ) -> Result<IndexResource, QueryError> {
        let started = Instant::now();
        let resource = kronika_index::resource_selected(
            self.reader.root(),
            &self.reader,
            Self::captured(segment)?,
            keys,
        )
        .map_err(|error| QueryError::Unreadable(Box::new(error)))?;
        let point_count = resource
            .index
            .blocks
            .iter()
            .map(index_block_len)
            .sum::<usize>();
        eprintln!(
            "kronika-web: index_resource segment_id={} logical_name={} persisted={} blocks={} points={} elapsed_us={}",
            segment.id(),
            logical_name,
            resource.persisted,
            resource.index.blocks.len(),
            point_count,
            started.elapsed().as_micros(),
        );
        Ok(IndexResource {
            index: resource.index,
        })
    }
}

const fn index_block_len(block: &kronika_index::SeriesBlock) -> usize {
    match block {
        kronika_index::SeriesBlock::OsHealth(points)
        | kronika_index::SeriesBlock::OverallHealth(points)
        | kronika_index::SeriesBlock::PostgresHealth(points) => points.len(),
        kronika_index::SeriesBlock::PgTransactions { points, .. } => points.len(),
        kronika_index::SeriesBlock::PgActiveBackends { points, .. } => points.len(),
        kronika_index::SeriesBlock::Findings(block) => block.findings.len(),
    }
}

fn log_catalog_open(segment_count: usize, listing: &Listing, started: Instant) {
    log_warnings(listing);
    eprintln!(
        "kronika-web: catalog_open segments={segment_count} warnings={} elapsed_us={}",
        listing.warnings.len(),
        started.elapsed().as_micros(),
    );
}

fn log_warnings(listing: &Listing) {
    for warning in &listing.warnings {
        eprintln!("kronika-web: store warning code={}", warning.reason.code());
    }
}

const fn warning(warning: kronika_reader::StoreWarning) -> DatasetWarning {
    let subject = match warning.affected {
        StoreObject::Segment(address) => DatasetWarningSubject::Segment(address.id.get()),
        StoreObject::ActiveJournal => DatasetWarningSubject::ActiveJournal,
        StoreObject::Foreign(path) => DatasetWarningSubject::ForeignEntry {
            name_hash: path.name_hash,
            name_len: path.name_len,
        },
        _ => DatasetWarningSubject::Other,
    };
    DatasetWarning {
        subject,
        code: warning.reason.code(),
    }
}
