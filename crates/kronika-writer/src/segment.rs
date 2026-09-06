//! Segment completion: merge the journal's parts into one immutable segment.
//!
//! Coalesces collection-window sections by type into a temporary file and
//! writes the end catalog last.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::FileExt as _;

use arrow_array::{
    Array, BinaryArray, BooleanArray, FixedSizeBinaryArray, RecordBatch, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use kronika_format::{
    Catalog, Crc32c, ENTRY_LEN, Entry, EntrySnapshot, FORMAT_VERSION, HotMark, MAGIC, META_LEN,
    PartError, Placement, Resolved, StrId, TAIL_INDEX_LEN, TailIndex, crc32c,
    validate_catalog_layout,
};
use kronika_layout::{FileIdentity, LayoutError, SegmentAddress, SegmentId, WriterOwner, ZmsTemp};
use kronika_registry::{
    Bytes, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_DECODED_SECTION_BYTES,
    MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS, VerifiedSection, contract,
    encode_final_sections_to, validate_plain_parquet_decode_work,
};
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};

use crate::{Journal, JournalError, JournalPartRef};

mod compare;
mod dictionary;
mod error;

#[cfg(test)]
use compare::arm_after_first_comparison_chunk;
use compare::{files_equal, validate_segment};
pub use dictionary::FinishedDictionary;
use dictionary::normalize_dictionary;
pub use error::WriteError;

const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
const COMPARE_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CATALOG_ENTRIES: usize = (MAX_CATALOG_BYTES - META_LEN) / ENTRY_LEN;

#[cfg(test)]
std::thread_local! {
    static AFTER_FIRST_COMPARISON_CHUNK:
        std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

/// Runs a test-only hook at one point of the byte comparison.
#[macro_export]
macro_rules! write_test_hook {
    (AfterFirstComparisonChunk) => {
        #[cfg(test)]
        run_after_first_comparison_chunk();
    };
}

/// What a completed segment contains, for the caller's metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteSummary {
    /// Number of catalog entries (sections) written.
    pub sections: usize,
    /// Total segment length, bytes.
    pub bytes: u64,
    /// Minimal timestamp across the segment, unix microseconds.
    pub min_ts: i64,
    /// Maximal timestamp across the segment, unix microseconds.
    pub max_ts: i64,
}

/// One already-final section body stored in a random-access spool.
///
/// The descriptor is independent of a ZMS offset. [`write_finished_zms`]
/// copies the body into canonical type order and writes the final catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishedSection {
    type_id: u32,
    rows: u32,
    offset: u64,
    len: u64,
    crc32c: u32,
}

impl FinishedSection {
    /// Describe one complete final Parquet body in a spool.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when the type, row count, or byte length is
    /// outside the finished-section envelope.
    pub fn new(
        type_id: u32,
        rows: u32,
        offset: u64,
        len: u64,
        crc32c: u32,
    ) -> Result<Self, WriteError> {
        if contract(type_id).is_none()
            && !matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID)
        {
            return Err(CodecError::UnknownType { type_id }.into());
        }
        let rows_usize = rows as usize;
        if rows_usize == 0 || rows_usize > MAX_SECTION_ROWS {
            return Err(CodecError::TooManyRows {
                rows: rows_usize,
                max: MAX_SECTION_ROWS,
            }
            .into());
        }
        check_final_section_len(len)?;
        offset
            .checked_add(len)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "spool section end",
            })?;
        Ok(Self {
            type_id,
            rows,
            offset,
            len,
            crc32c,
        })
    }

    /// Registered or dictionary type id.
    #[must_use]
    pub const fn type_id(self) -> u32 {
        self.type_id
    }

    /// Rows encoded in the body.
    #[must_use]
    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// Body start in the spool.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }

    /// Encoded body length.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.len
    }

    /// Whether the body is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// CRC32C of the complete body.
    #[must_use]
    pub const fn crc32c(self) -> u32 {
        self.crc32c
    }
}

/// Complete metadata for one finished ZMS assembled from spooled sections.
#[derive(Debug)]
pub struct FinishedZmsPlan {
    sections: Vec<FinishedSection>,
    min_ts: i64,
    max_ts: i64,
    window_count: u32,
}

impl FinishedZmsPlan {
    /// Validate and canonicalize a standalone segment plan.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] for inverted timestamps or duplicate section
    /// types.
    pub fn new(
        mut sections: Vec<FinishedSection>,
        min_ts: i64,
        max_ts: i64,
        window_count: u32,
    ) -> Result<Self, WriteError> {
        if min_ts > max_ts {
            return Err(WriteError::InvalidTimestampBounds { min_ts, max_ts });
        }
        checked_catalog_entries(0, sections.len())?;
        sections.sort_unstable_by_key(|section| section.type_id);
        if sections
            .windows(2)
            .any(|pair| pair[0].type_id == pair[1].type_id)
        {
            return Err(CodecError::SchemaMismatch.into());
        }
        Ok(Self {
            sections,
            min_ts,
            max_ts,
            window_count,
        })
    }

    /// Canonically ordered section descriptors.
    #[must_use]
    pub fn sections(&self) -> &[FinishedSection] {
        &self.sections
    }

    /// Minimal timestamp carried by selected rows.
    #[must_use]
    pub const fn min_ts(&self) -> i64 {
        self.min_ts
    }

    /// Maximal timestamp carried by selected rows.
    #[must_use]
    pub const fn max_ts(&self) -> i64 {
        self.max_ts
    }

    /// Source collection-window count recorded in the catalog.
    #[must_use]
    pub const fn window_count(&self) -> u32 {
        self.window_count
    }
}

/// Assemble one standalone finished ZMS from already-final section bodies.
///
/// `spool` is read only at the ranges described by `plan`. The output is
/// written from its current position and flushed before return. Section bytes
/// and checksums are verified while copying.
///
/// # Errors
///
/// Returns [`WriteError`] when a spool range cannot be read, a checksum does
/// not match, or writing the container fails.
pub fn write_finished_zms(
    spool: &File,
    plan: &FinishedZmsPlan,
    out: &mut impl Write,
) -> Result<WriteSummary, WriteError> {
    let mut out = BufWriter::new(out);
    let summary = write_finished_zms_core(spool, plan, &mut out)?;
    out.flush()?;
    Ok(summary)
}

/// Write journal parts into the immutable segment at `address`.
///
/// The final ZMS is never overwritten. Call `Journal::reset` only after `Ok`.
///
/// # Errors
///
/// Returns [`WriteError`] when the journal is empty, a part is invalid, I/O
/// fails, or an existing final segment cannot be proven byte-identical.
pub fn write_segment(
    journal: &Journal,
    owner: &WriterOwner,
    address: SegmentAddress,
) -> Result<WriteSummary, WriteError> {
    if journal.parts().is_empty() {
        return Err(WriteError::Empty);
    }
    if let Some(segment_id) = journal.segment_id()
        && segment_id != address.id
    {
        return Err(WriteError::SegmentIdMismatch {
            journal: segment_id,
            destination: address.id,
        });
    }
    let mut temporary = owner.create_zms_temp(address)?;
    let mut spool = owner.create_zms_temp(address)?;
    let summary = write_tmp(journal, &mut temporary, &mut spool)?;
    spool.discard()?;
    let generated = temporary.try_clone_file()?;
    if !validate_segment(&generated, summary)? {
        return Err(WriteError::GeneratedSegmentInvalid);
    }
    match temporary.publish() {
        Ok(()) => Ok(summary),
        Err(LayoutError::SegmentAlreadyExists { .. }) => {
            let existing = owner.root().open_zms(address)?;
            let existing_identity = FileIdentity::from_file(&existing)?;
            if !validate_segment(&existing, summary)? {
                return Err(WriteError::ExistingSegmentInvalid);
            }
            if !files_equal(&generated, &existing)? {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            if FileIdentity::from_file(&existing)? != existing_identity {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            let named_existing = owner.root().open_zms(address)?;
            if FileIdentity::from_file(&named_existing)? != existing_identity {
                return Err(WriteError::ExistingSegmentMismatch);
            }
            temporary.discard()?;
            Ok(summary)
        }
        Err(error) => Err(WriteError::Layout(error)),
    }
}

fn checked_catalog_entries(current: usize, additional: usize) -> Result<usize, WriteError> {
    let attempted_entries = current
        .checked_add(additional)
        .ok_or(WriteError::CatalogTooLarge {
            attempted_entries: usize::MAX,
            max_entries: MAX_CATALOG_ENTRIES,
        })?;
    if attempted_entries > MAX_CATALOG_ENTRIES {
        return Err(WriteError::CatalogTooLarge {
            attempted_entries,
            max_entries: MAX_CATALOG_ENTRIES,
        });
    }
    Ok(attempted_entries)
}

#[derive(Debug, Clone, Copy)]
struct SectionDescriptor {
    part: JournalPartRef,
    entry: Entry,
}

#[derive(Debug)]
struct SegmentPlan {
    by_type: BTreeMap<u32, Vec<SectionDescriptor>>,
    min_ts: i64,
    max_ts: i64,
    window_count: u32,
}

/// Write the merged segment to `tmp` and flush the encoder.
///
/// Publication synchronizes the file and its parent directories.
fn write_tmp(
    journal: &Journal,
    temporary: &mut ZmsTemp<'_>,
    spool: &mut ZmsTemp<'_>,
) -> Result<WriteSummary, WriteError> {
    let mut plan = plan_segment(journal)?;
    let strings = plan
        .by_type
        .remove(&DICT_STRINGS_TYPE_ID)
        .unwrap_or_default();
    let blobs = plan.by_type.remove(&DICT_BLOBS_TYPE_ID).unwrap_or_default();

    let mut types = plan.by_type.into_iter().collect::<Vec<_>>();
    types.sort_by_key(|(type_id, descriptors)| {
        let bytes = descriptors.iter().fold(0_u64, |total, descriptor| {
            total.saturating_add(descriptor.entry.len)
        });
        (Reverse(bytes), *type_id)
    });
    let mut spool_out = BufWriter::new(spool.file_mut());
    let mut spooled = Vec::new();
    let mut spool_offset = 0_u64;

    for (type_id, descriptors) in types {
        let section =
            spool_data_section(journal, type_id, &descriptors, &mut spool_out, spool_offset)?;
        spool_offset =
            spool_offset
                .checked_add(section.len)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "spool offset",
                })?;
        spooled.push(section);
    }

    spool_dictionary_sections(
        journal,
        &strings,
        &blobs,
        &mut spool_out,
        &mut spooled,
        &mut spool_offset,
    )?;
    let spool_file = spool_out
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?;
    let finished = FinishedZmsPlan::new(spooled, plan.min_ts, plan.max_ts, plan.window_count)?;
    let summary = write_finished_zms(spool_file, &finished, temporary.file_mut())?;
    temporary.file_mut().sync_all()?;
    Ok(summary)
}

fn write_finished_zms_core(
    spool: &File,
    plan: &FinishedZmsPlan,
    out: &mut impl Write,
) -> Result<WriteSummary, WriteError> {
    out.write_all(&MAGIC)?;
    let mut offset = MAGIC.len() as u64;
    let mut entries: Vec<Entry> = Vec::new();
    let mut copy_buffer = vec![0_u8; COMPARE_BUFFER_BYTES].into_boxed_slice();
    for &section in &plan.sections {
        copy_spooled_section(spool, out, section, &mut copy_buffer)?;
        push_section_entry(&mut entries, &mut offset, section)?;
    }

    let sections = entries.len();
    let catalog = Catalog {
        entries,
        min_ts: plan.min_ts,
        max_ts: plan.max_ts,
        format_version: FORMAT_VERSION,
        window_count: plan.window_count,
    };
    let catalog_bytes = catalog.encoded_len().checked_add(TAIL_INDEX_LEN).ok_or(
        WriteError::ArithmeticOverflow {
            what: "finished catalog length",
        },
    )?;
    let bytes = offset
        .checked_add(catalog_bytes as u64)
        .ok_or(WriteError::ArithmeticOverflow {
            what: "finished segment length",
        })?;
    catalog.write_encoded(out)?;

    Ok(WriteSummary {
        sections,
        bytes,
        min_ts: plan.min_ts,
        max_ts: plan.max_ts,
    })
}

fn spool_data_section(
    journal: &Journal,
    type_id: u32,
    descriptors: &[SectionDescriptor],
    out: &mut (impl Write + Send),
    offset: u64,
) -> Result<FinishedSection, WriteError> {
    let declared_rows = aggregate_rows(type_id, descriptors)?;
    let rows = descriptors
        .iter()
        .map(|descriptor| descriptor.entry.rows)
        .collect::<Vec<_>>();
    let mut verified = vec![false; descriptors.len()];
    let mut sink = SectionSink::new(out);
    encode_final_sections_to(
        type_id,
        &rows,
        &mut sink,
        |index| -> Result<_, WriteError> {
            let descriptor = descriptors
                .get(index)
                .copied()
                .ok_or(CodecError::SchemaMismatch)?;
            if verified[index] {
                Ok(Bytes::from(read_section_body(journal, descriptor)?))
            } else {
                verified[index] = true;
                Ok(read_verified_body(journal, descriptor)?.into_bytes())
            }
        },
    )?;
    let (len, checksum) = sink.finish();
    check_final_section_len(len)?;
    FinishedSection::new(
        type_id,
        u32::try_from(declared_rows).map_err(|_overflow| WriteError::ArithmeticOverflow {
            what: "section row count",
        })?,
        offset,
        len,
        checksum,
    )
}

fn spool_dictionary_sections(
    journal: &Journal,
    strings: &[SectionDescriptor],
    blobs: &[SectionDescriptor],
    out: &mut (impl Write + Send),
    spooled: &mut Vec<FinishedSection>,
    offset: &mut u64,
) -> Result<(), WriteError> {
    let dictionary = normalize_dictionary(journal, strings, blobs)?;
    for section in dictionary.write_sections_to(out, *offset)? {
        *offset =
            section
                .offset
                .checked_add(section.len)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "spool offset",
                })?;
        spooled.push(section);
    }
    Ok(())
}

struct SectionSink<W> {
    out: W,
    len: u64,
    checksum: Crc32c,
}

impl<W> SectionSink<W> {
    const fn new(out: W) -> Self {
        Self {
            out,
            len: 0,
            checksum: Crc32c::new(),
        }
    }

    fn finish(self) -> (u64, u32) {
        (self.len, self.checksum.finalize())
    }
}

impl<W: Write> Write for SectionSink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.out.write(buf)?;
        self.checksum.update(&buf[..written]);
        self.len = self
            .len
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "section length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

fn check_final_section_len(len: u64) -> Result<(), WriteError> {
    let len_usize = usize::try_from(len).map_err(|_overflow| CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    })?;
    if len_usize == 0 || len_usize > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: len_usize,
            max: MAX_SECTION_BYTES,
        }
        .into());
    }
    Ok(())
}

fn copy_spooled_section(
    spool: &File,
    out: &mut impl Write,
    section: FinishedSection,
    buffer: &mut [u8],
) -> Result<(), WriteError> {
    let mut copied = 0_u64;
    let mut checksum = Crc32c::new();
    while copied < section.len {
        let remaining = usize::try_from(section.len - copied).unwrap_or(usize::MAX);
        let chunk = remaining.min(buffer.len());
        let at = section
            .offset
            .checked_add(copied)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "spool read offset",
            })?;
        spool.read_exact_at(&mut buffer[..chunk], at)?;
        out.write_all(&buffer[..chunk])?;
        checksum.update(&buffer[..chunk]);
        copied = copied
            .checked_add(chunk as u64)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "spool copy length",
            })?;
    }
    let got = checksum.finalize();
    if got != section.crc32c {
        return Err(CodecError::SectionCrcMismatch {
            expected: section.crc32c,
            got,
        }
        .into());
    }
    Ok(())
}

fn push_section_entry(
    entries: &mut Vec<Entry>,
    offset: &mut u64,
    section: FinishedSection,
) -> Result<(), WriteError> {
    if entries
        .last()
        .is_some_and(|entry| entry.type_id >= section.type_id)
    {
        return Err(CodecError::SchemaMismatch.into());
    }
    checked_catalog_entries(entries.len(), 1)?;
    entries
        .try_reserve(1)
        .map_err(WriteError::CatalogAllocation)?;
    entries.push(Entry {
        type_id: section.type_id,
        flags: 0,
        offset: *offset,
        len: section.len,
        rows: section.rows,
        crc32c: section.crc32c,
    });
    *offset = offset
        .checked_add(section.len)
        .ok_or(WriteError::ArithmeticOverflow {
            what: "segment offset",
        })?;
    Ok(())
}

fn plan_segment(journal: &Journal) -> Result<SegmentPlan, WriteError> {
    let mut by_type = BTreeMap::<u32, Vec<SectionDescriptor>>::new();
    let mut section_count = 0_usize;
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    let window_count =
        u32::try_from(journal.parts().len()).map_err(|_error| WriteError::ArithmeticOverflow {
            what: "window count",
        })?;
    for &part_ref in journal.parts() {
        // Recheck framing immediately before publication. Each body is CRC
        // checked separately just before its type is finalized.
        let catalog = read_part_catalog(journal, part_ref)?;
        if catalog.format_version != FORMAT_VERSION {
            return Err(WriteError::UnsupportedFormat {
                version: catalog.format_version,
            });
        }
        min_ts = min_ts.min(catalog.min_ts);
        max_ts = max_ts.max(catalog.max_ts);
        for entry in catalog.entries {
            section_count = section_count
                .checked_add(1)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "section descriptor count",
                })?;
            if section_count > MAX_SECTION_ROWS {
                return Err(WriteError::TooManySections {
                    sections: section_count,
                    max: MAX_SECTION_ROWS,
                });
            }
            let descriptors = by_type.entry(entry.type_id).or_default();
            descriptors
                .try_reserve(1)
                .map_err(WriteError::CatalogAllocation)?;
            descriptors.push(SectionDescriptor {
                part: part_ref,
                entry,
            });
        }
    }
    if min_ts > max_ts {
        min_ts = 0;
        max_ts = 0;
    }
    Ok(SegmentPlan {
        by_type,
        min_ts,
        max_ts,
        window_count,
    })
}

fn read_part_catalog(journal: &Journal, part: JournalPartRef) -> Result<Catalog, WriteError> {
    let minimum = MAGIC.len() + META_LEN + TAIL_INDEX_LEN;
    if part.len() < minimum {
        return Err(WriteError::Part(PartError::TooShort { actual: part.len() }));
    }
    let magic = journal.read_part_range(part, 0, MAGIC.len())?;
    if magic.as_slice() != MAGIC {
        let mut actual = [0_u8; 4];
        actual.copy_from_slice(&magic);
        return Err(WriteError::Part(PartError::BadMagic { actual }));
    }
    let tail_at = part.len() - TAIL_INDEX_LEN;
    let tail = journal.read_part_range(part, tail_at, TAIL_INDEX_LEN)?;
    let tail: [u8; TAIL_INDEX_LEN] = tail
        .try_into()
        .map_err(|_bytes| WriteError::Part(PartError::TooShort { actual: part.len() }))?;
    let tail = TailIndex::decode(tail).map_err(|error| WriteError::Part(PartError::Tail(error)))?;
    let catalog_len = tail.catalog_len as usize;
    let Some(catalog_at) = tail_at.checked_sub(catalog_len) else {
        return Err(WriteError::Part(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        }));
    };
    if catalog_at < MAGIC.len() {
        return Err(WriteError::Part(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        }));
    }
    let bytes = journal.read_part_range(part, catalog_at, catalog_len)?;
    let catalog =
        Catalog::decode(&bytes).map_err(|error| WriteError::Part(PartError::Catalog(error)))?;
    validate_catalog_layout(&catalog, catalog_at as u64)
        .map_err(|error| WriteError::Part(PartError::Layout(error)))?;
    Ok(catalog)
}

fn aggregate_rows(type_id: u32, descriptors: &[SectionDescriptor]) -> Result<usize, WriteError> {
    let rows = descriptors.iter().try_fold(0_usize, |rows, descriptor| {
        rows.checked_add(descriptor.entry.rows as usize)
            .ok_or(WriteError::ArithmeticOverflow {
                what: "section row count",
            })
    })?;
    if rows > MAX_SECTION_ROWS {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        }
        .into());
    }
    if type_id == 0 {
        return Err(CodecError::UnknownType { type_id }.into());
    }
    Ok(rows)
}

fn read_verified_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
) -> Result<VerifiedSection, WriteError> {
    let start = usize::try_from(descriptor.entry.offset).map_err(|_error| {
        WriteError::ArithmeticOverflow {
            what: "section offset",
        }
    })?;
    let len =
        usize::try_from(descriptor.entry.len).map_err(|_error| WriteError::ArithmeticOverflow {
            what: "section length",
        })?;
    let body = journal.read_part_range(descriptor.part, start, len)?;
    VerifiedSection::verify(Bytes::from(body), descriptor.entry.crc32c, crc32c)
        .map_err(WriteError::Codec)
}

fn read_section_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
) -> Result<Vec<u8>, WriteError> {
    let start = usize::try_from(descriptor.entry.offset).map_err(|_overflow| {
        WriteError::ArithmeticOverflow {
            what: "section offset",
        }
    })?;
    let len = usize::try_from(descriptor.entry.len).map_err(|_overflow| {
        WriteError::ArithmeticOverflow {
            what: "section length",
        }
    })?;
    journal
        .read_part_range(descriptor.part, start, len)
        .map_err(WriteError::Journal)
}

#[cfg(test)]
mod tests;
