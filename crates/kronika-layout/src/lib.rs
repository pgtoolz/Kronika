//! Typed access to the `Kronika` data-directory layout.
//!
//! Finished segments live at `YYYY/MM/DD/N.zms`, where the UTC bucket is
//! derived from [`SegmentId`]. Derived index files use the sibling
//! `N.idx` name. [`DataRoot`] validates the closed directory grammar and opens
//! every component relative to an already-open directory descriptor without
//! following symbolic links.

mod error;
mod limits;
#[cfg(feature = "posix")]
mod root;
mod time;

pub use error::{LayoutError, LimitKind, OwnerKind};
pub use limits::LayoutLimits;
#[cfg(feature = "posix")]
pub use root::{
    ACTIVE_JOURNAL_NAME, DAMAGED_JOURNAL_NAME, DataRoot, DayDirectory, EntryDiagnostic,
    EntryFileType, EntryScope, FileIdentity, FileKind, FilesystemUsage, ForeignEntry,
    ForeignEntryReason, INDEX_OWNER_LOCK_NAME, IdxTemp, IndexOwner, LOG_OFFSETS_NAME,
    LOG_OFFSETS_TEMP_NAME, LayoutSnapshot, PathIdentity, SegmentArtifacts, SegmentRemoval,
    TemporaryKind, TemporaryObject, WRITER_OWNER_LOCK_NAME, WriterLease, WriterOwner, ZmsTemp,
};
pub use time::{SegmentAddress, SegmentId, UtcDay};
