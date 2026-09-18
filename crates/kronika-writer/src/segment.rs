//! Segment completion: merge the journal's parts into one immutable segment.
//!
//! Coalesces collection-window sections by type into a temporary file and
//! writes the end catalog last.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::FileExt as _;

use kronika_format::{
    Catalog, Crc32c, ENTRY_LEN, Entry, FORMAT_VERSION, MAGIC, META_LEN, TAIL_INDEX_LEN,
};
use kronika_layout::{FileIdentity, LayoutError, SegmentAddress, WriterOwner};
use kronika_registry::{
    CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    contract,
};

use crate::Journal;

mod compare;
mod dictionary;
mod error;
mod plan;
mod spool;

#[cfg(test)]
use compare::arm_after_first_comparison_chunk;
use compare::{files_equal, validate_segment};
pub use dictionary::FinishedDictionary;
use dictionary::normalize_dictionary;
pub use error::WriteError;
use spool::write_tmp;

// Cap catalog allocation independently of segment-body size (64 MiB).
const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;
// A fixed 64 KiB buffer bounds spool-copy and recovery-comparison memory.
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

#[cfg(test)]
#[path = "tests/segment.rs"]
mod tests;
