//! Bounded writer state, the journal, and segment writing.
//!
//! [`SectionBuffers`] accepts registered rows until the registry row cap,
//! encodes one collection window, and places data sections before dictionary
//! sections. [`Interner`] owns the current segment's dictionary: unflushed
//! values retain their bytes, while flushed values retain only identity and
//! placement metadata for deduplication.
//!
//! [`Journal`] appends self-contained ZMS parts as synchronized `ZMSP` frames
//! after a checksummed version-1 header in `active.wal`. Opening validates
//! the complete header and body without repairing or truncating damage.
//! [`JournalConfig::max_journal_len`] is the hard growth bound, reported as
//! [`JournalError::Full`] so the collector can close the segment early.
//!
//! [`write_segment`] validates and decodes journal bodies, coalesces each
//! registered type, normalizes dictionaries, and emits canonical Parquet 1.0
//! bodies with PLAIN values and Zstandard level 6. It writes a temporary file
//! in the segment's UTC day and publishes without overwriting another
//! identity. A retry accepts an existing final file only after exact
//! comparison. Writing never resets the journal, so the caller does so only
//! after `Ok`.

mod buffer;
pub mod dict;
mod interner;
mod journal;
mod segment;

pub use buffer::{FlushSummary, FlushedPart, SectionBuffers, SectionFlushSummary};
pub use interner::{FinishedSegment, FlushedEntry, Interner};
pub use journal::{Journal, JournalConfig, JournalError, JournalPartRef};
pub use kronika_format::{MAX_JOURNAL_LEN, MAX_JOURNAL_PARTS, MAX_PART_LEN};
pub use segment::{
    FinishedDictionary, FinishedSection, FinishedZmsPlan, WriteError, WriteSummary,
    write_finished_zms, write_segment,
};

#[cfg(test)]
#[path = "tests/composition.rs"]
mod composition_tests;
