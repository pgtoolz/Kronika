//! One finished or current logical segment, opened.

use std::collections::BTreeMap;
use std::sync::Arc;

use kronika_format::{Catalog, Entry, ReadAt, crc32c};
use kronika_registry::{Bytes, RecordBatch, Row, VerifiedSection, contract, logical_section_name};
use kronika_store::CatalogSummary;
#[cfg(feature = "posix")]
use kronika_store::{ActiveSnapshot, LocalDir};

use crate::SegmentKind;
use crate::error::ReaderError;
#[cfg(feature = "posix")]
use crate::{SegmentRef, SegmentSource};

mod dictionary;
mod rows;

trait SegmentBytes: ReadAt + Send + Sync {}

impl<T: ReadAt + Send + Sync> SegmentBytes for T {}

// Type erasure stays private and occurs only after a generic source opens one
// object, keeping the public product segment common to POSIX, embedded, and
// captured active bytes.
struct OpenedSegmentBytes(Box<dyn SegmentBytes>);

impl OpenedSegmentBytes {
    fn new(bytes: impl ReadAt + Send + Sync + 'static) -> Self {
        Self(Box::new(bytes))
    }
}

impl std::fmt::Debug for OpenedSegmentBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedSegmentBytes").finish_non_exhaustive()
    }
}

impl ReadAt for OpenedSegmentBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        self.0.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> std::io::Result<u64> {
        self.0.byte_len()
    }
}

#[derive(Debug)]
enum Source {
    Finished {
        bytes: OpenedSegmentBytes,
        catalog: Arc<Catalog>,
    },
    #[cfg(feature = "posix")]
    Active(ActiveSnapshot),
}

/// An open finished `.zms` or captured `active.wal` logical segment.
///
/// Section bodies are read on demand. A finished segment holds one positional
/// byte source; a current segment holds one descriptor for its captured prefix.
#[derive(Debug)]
pub struct Segment {
    id: i64,
    kind: SegmentKind,
    source_label: String,
    source: Source,
    min_ts: i64,
    max_ts: i64,
    captured_bytes: u64,
    window_count: u32,
    section_rows: BTreeMap<u32, Section>,
}

/// What one section type occupies in a segment.
///
/// A current segment coalesces several parts, so both figures are the sum over
/// the parts that carry the type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Section {
    /// Rows the catalogs recorded.
    pub rows: u64,
    /// Encoded body bytes, before the container's own framing.
    pub bytes: u64,
}

impl Segment {
    #[cfg(feature = "posix")]
    pub(crate) fn open(
        dir: &LocalDir,
        unit: &SegmentRef,
        source_label: String,
    ) -> Result<Self, ReaderError> {
        match &unit.source {
            SegmentSource::Finished(finished) => {
                let file = dir.open_finished(finished)?;
                let catalog = Arc::new(kronika_store::read_catalog(&file)?);
                dir.validate_finished_file(&file, finished)?;
                Ok(Self::open_finished(
                    file,
                    catalog,
                    unit.segment_id,
                    unit.captured_bytes,
                    &finished.summary,
                    source_label,
                ))
            }
            SegmentSource::Active(snapshot) => {
                let window_count = u32::try_from(snapshot.parts().len()).map_err(|_overflow| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "active segment window count does not fit u32",
                    )
                })?;
                let section_rows = rows_by_type(snapshot.parts().iter().map(|part| &part.catalog));
                Ok(Self {
                    id: unit.segment_id,
                    kind: SegmentKind::Active,
                    source_label,
                    source: Source::Active(snapshot.clone()),
                    min_ts: unit.min_ts,
                    max_ts: unit.max_ts,
                    captured_bytes: unit.captured_bytes,
                    window_count,
                    section_rows,
                })
            }
        }
    }

    pub(crate) fn open_finished(
        bytes: impl ReadAt + Send + Sync + 'static,
        catalog: Arc<Catalog>,
        segment_id: i64,
        captured_bytes: u64,
        summary: &CatalogSummary,
        source_label: String,
    ) -> Self {
        let section_rows = rows_by_type(std::iter::once(catalog.as_ref()));
        Self {
            id: segment_id,
            kind: SegmentKind::Finished,
            source_label,
            source: Source::Finished {
                bytes: OpenedSegmentBytes::new(bytes),
                catalog,
            },
            min_ts: summary.min_ts,
            max_ts: summary.max_ts,
            captured_bytes,
            window_count: summary.window_count,
            section_rows,
        }
    }

    /// Stable segment id.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Whether this segment is immutable or a captured journal prefix.
    #[must_use]
    pub const fn kind(&self) -> SegmentKind {
        self.kind
    }

    /// Exact committed active-journal position, when current.
    #[must_use]
    pub const fn active_position(&self) -> Option<u64> {
        match self.kind {
            SegmentKind::Finished => None,
            SegmentKind::Active => Some(self.captured_bytes),
        }
    }

    /// Storage-neutral display label for the source object.
    ///
    /// The native directory reader supplies the same path text exposed by the
    /// previous API. Other sources use their logical resource identity.
    #[must_use]
    pub fn source_label(&self) -> &str {
        &self.source_label
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

    /// Bytes in the finished object or captured current-journal prefix.
    #[must_use]
    pub const fn captured_bytes(&self) -> u64 {
        self.captured_bytes
    }

    /// Collection windows coalesced into the logical segment.
    #[must_use]
    pub const fn window_count(&self) -> u32 {
        self.window_count
    }

    /// Section types present in the segment, in numeric order.
    pub fn type_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.section_rows.keys().copied()
    }

    /// Rows recorded for `type_id`, or `None` when the section is absent.
    #[must_use]
    pub fn rows_of(&self, type_id: u32) -> Option<u64> {
        self.section_rows.get(&type_id).map(|section| section.rows)
    }

    /// Every section the segment carries, in numeric order, with what it cost.
    ///
    /// This is what a size report is built from: the bodies are the segment
    /// minus its catalog and framing.
    pub fn sections(&self) -> impl Iterator<Item = (u32, Section)> + '_ {
        self.section_rows
            .iter()
            .map(|(type_id, section)| (*type_id, *section))
    }

    /// Physical layouts present under one stable public section name.
    pub fn layouts<'a>(
        &'a self,
        logical_name: &'a str,
    ) -> impl Iterator<Item = (u32, Section)> + 'a {
        self.sections().filter(move |(type_id, _section)| {
            contract(*type_id).is_some()
                && logical_section_name(*type_id).is_some_and(|name| name == logical_name)
        })
    }
}

#[allow(
    single_use_lifetimes,
    reason = "the named lifetime is required in this impl-Trait associated item on Rust 1.96"
)]
fn rows_by_type<'a>(catalogs: impl IntoIterator<Item = &'a Catalog>) -> BTreeMap<u32, Section> {
    let mut sections: BTreeMap<u32, Section> = BTreeMap::new();
    for catalog in catalogs {
        for entry in &catalog.entries {
            let section = sections.entry(entry.type_id).or_default();
            section.rows += u64::from(entry.rows);
            section.bytes += entry.len;
        }
    }
    sections
}

fn entry(catalog: &Catalog, type_id: u32) -> Option<&Entry> {
    catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == type_id)
}

fn finished_body(bytes: &impl ReadAt, entry: &Entry) -> Result<VerifiedSection, ReaderError> {
    let len = usize::try_from(entry.len).map_err(|_overflow| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("section {} declares {} bytes", entry.type_id, entry.len),
        )
    })?;
    let mut buffer = vec![0_u8; len];
    bytes.read_exact_at(&mut buffer, entry.offset)?;
    verified_body(entry, buffer)
}

#[cfg(feature = "posix")]
fn active_body(
    snapshot: &ActiveSnapshot,
    part_index: usize,
    entry: &Entry,
) -> Result<VerifiedSection, ReaderError> {
    let buffer = snapshot.read_part_range(part_index, entry.offset, entry.len)?;
    verified_body(entry, buffer)
}

fn verified_body(entry: &Entry, body: Vec<u8>) -> Result<VerifiedSection, ReaderError> {
    VerifiedSection::verify(Bytes::from(body), entry.crc32c, crc32c).map_err(|source| {
        ReaderError::Section {
            type_id: entry.type_id,
            source,
        }
    })
}
