//! `active.wal` journal frames.
//!
//! File I/O is in `kronika-writer`. This module defines frame bytes and
//! in-memory recovery.

use std::error::Error;
use std::fmt;
use std::io;

use crate::{
    Catalog, CatalogLayoutError, DecodeError, Entry, MAGIC, ReadAt, TAIL_INDEX_LEN, TailIndex,
    crc32c, validate_catalog_layout,
};

mod frame;
mod header;
mod scan;

pub use frame::{
    FrameError, FrameHeader, PartError, PartMeta, SectionInput, build_part, validate_part,
    validate_part_catalog,
};
pub use header::{
    JournalHeader, JournalHeaderError, JournalState, ResetHeaderTransition, ResetMarker,
};
pub use scan::{
    JournalLimits, JournalScanError, PartRef, ScanReport, scan_journal_streaming_strict_from,
    visit_journal_streaming_strict_from,
};

/// Magic bytes opening every journal frame.
pub const FRAME_MAGIC: [u8; 4] = *b"ZMSP";

/// File signature for the only supported active-journal header format.
pub const JOURNAL_MAGIC: [u8; 8] = *b"KRNJNL1\0";

/// Current active-journal format version.
pub const JOURNAL_VERSION: u32 = 1;

/// Size of the version-1 active-journal header.
pub const JOURNAL_HEADER_LEN: usize = 36;

/// Magic bytes opening a committed journal-reset marker.
pub const RESET_MARKER_MAGIC: [u8; 8] = *b"KRNRST1\0";

/// Size of a committed journal-reset marker.
pub const RESET_MARKER_LEN: usize = 32;

/// Size of a frame header on disk, bytes.
pub const FRAME_HEADER_LEN: usize = 16;

/// Hard version-1 admission limit for the complete active journal, bytes.
pub const MAX_JOURNAL_LEN: usize = 1024 * 1024 * 1024;

/// Hard version-1 admission limit for one part body, bytes.
pub const MAX_PART_LEN: u64 = 64 * 1024 * 1024;

/// Hard version-1 admission limit for valid frames in one active journal.
pub const MAX_JOURNAL_PARTS: usize = 1_000_000;

/// Fixed read-buffer size used by the streaming recovery scanner.
///
/// Candidate part bodies are separately bounded by [`JournalLimits`].
pub const RECOVERY_SCAN_CHUNK_LEN: usize = 64 * 1024;

#[cfg(test)]
mod streaming_tests;

#[cfg(test)]
mod tests;
