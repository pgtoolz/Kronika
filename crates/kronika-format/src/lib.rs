//! Byte-level pieces of a ZMS segment.
//!
//! The public types define the file structures that writers and readers share
//! exactly: the end catalog, tail index, CRC32C checksums, `str_id`,
//! per-segment dictionaries, and the `active.wal` journal frame format.
//!
//! The crate does not encode Parquet sections, access storage, or know the
//! meaning of a section body. Higher-level crates handle those jobs.
//!
//! To open a segment, start at the end of the file:
//!
//! 1. Decode the last [`TAIL_INDEX_LEN`] bytes with [`TailIndex::decode`].
//! 2. Read the catalog bytes immediately before the tail index.
//! 3. Decode them with [`Catalog::decode`].
//! 4. Read section bodies using `offset` and `len` from [`Entry`].
//!
//! CRC32C detects accidental corruption before higher-level decoding. It is
//! not a signature and does not make a segment trustworthy against deliberate
//! modification.

mod catalog;
mod crc;
mod dictionary;
mod parts;
mod read_at;
mod str_id;

// proptest and tempfile are used by external test files only; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
#[cfg(test)]
use proptest as _;
#[cfg(test)]
use tempfile as _;

pub use catalog::{
    Catalog, CatalogLayoutError, CatalogView, DecodeError, ENTRY_LEN, Entry,
    MAX_PHYSICAL_SECTION_BYTES, META_LEN, TAIL_INDEX_LEN, TailIndex, validate_catalog_layout,
};
pub use crc::{Crc32c, crc32c};
pub use dictionary::{
    BlobEntry, DEFAULT_BLOB_THRESHOLD, DEFAULT_MAX_TOTAL_BYTES, DEFAULT_TRUNCATE_LIMIT, DictError,
    DictLimits, DictStats, InvalidLimits, Resolved, SegmentDicts,
};
pub use dictionary::{EntrySnapshot, HotMark, Placement};
pub use parts::{
    FRAME_HEADER_LEN, FRAME_MAGIC, FrameError, FrameHeader, JOURNAL_HEADER_LEN, JOURNAL_MAGIC,
    JOURNAL_VERSION, JournalHeader, JournalHeaderError, JournalLimits, JournalScanError,
    JournalState, MAX_JOURNAL_LEN, MAX_JOURNAL_PARTS, MAX_PART_LEN, PartError, PartMeta, PartRef,
    RECOVERY_SCAN_CHUNK_LEN, RESET_MARKER_LEN, RESET_MARKER_MAGIC, ResetHeaderTransition,
    ResetMarker, ScanReport, SectionInput, build_part, scan_journal_streaming_strict_from,
    validate_part, validate_part_catalog, visit_journal_streaming_strict_from,
};
pub use read_at::ReadAt;
pub use str_id::StrId;

/// Segment magic bytes.
///
/// A ZMS file starts with `ZMS1`, and the tail index also ends with `ZMS1`.
/// Readers use the second copy when opening a segment from the end.
pub const MAGIC: [u8; 4] = *b"ZMS1";

/// Version of the container layout.
///
/// This number changes only when the container framing changes. Section
/// schemas evolve through new `type_id` values.
pub const FORMAT_VERSION: u32 = 1;
