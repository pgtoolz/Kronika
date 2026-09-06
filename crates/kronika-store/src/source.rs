//! Storage unit types returned by the store scan.

use std::fs::File;
use std::sync::Arc;

use kronika_format::{Catalog, PartRef};
use kronika_layout::{
    FileIdentity, ForeignEntryReason, LayoutError, PathIdentity, SegmentAddress, SegmentId,
};

use crate::{CatalogDigest, CatalogSummary};

/// A finished `.zms` segment pinned to one filesystem identity.
///
/// Discovery retains only a compact catalog summary. Consumers open the full
/// catalog lazily after [`super::LocalDir::open_finished`] verifies that the
/// ZMS at the verified [`SegmentAddress`] still has this identity.
#[derive(Debug, Clone)]
pub struct FinalUnit {
    /// Verified logical and physical address.
    pub address: SegmentAddress,
    /// Exact filesystem identity observed by the strict layout traversal and
    /// revalidated around the catalog read.
    pub identity: FileIdentity,
    /// Fixed-size validated catalog metadata shared across cached scans.
    pub summary: Arc<CatalogSummary>,
}

/// One valid part from the `active.wal` journal.
///
/// The catalog was decoded from the part bytes but the section bodies remain
/// unread.
#[derive(Debug, Clone)]
pub struct ActivePart {
    /// Identity of the active segment persisted in journal v1.
    pub segment_id: SegmentId,
    /// Location of the part body inside the journal file.
    pub part: PartRef,
    /// Catalog decoded from the part bytes.
    pub catalog: Catalog,
    /// Offset-independent identity derived when the catalog was validated.
    pub catalog_digest: CatalogDigest,
}

/// A complete, internally consistent scan of `active.wal`.
///
/// This value can be captured under a journal identity handshake and completed
/// with a finished-tree scan later via [`super::LocalDir::complete_scan`].
#[derive(Debug, Clone)]
pub struct JournalScan {
    /// Valid parts from `active.wal`, in journal order.
    #[expect(
        clippy::rc_buffer,
        reason = "incremental readers must share an unchanged active baseline without copying catalogs"
    )]
    pub active: Arc<Vec<ActivePart>>,
    /// Byte offset of the end of the complete physical journal state.
    pub valid_len: u64,
    /// Whether the journal contains a committed reset marker.
    ///
    /// All marker publication phases are logically empty even though the old
    /// frames remain present and are validated by the scan.
    pub committed_reset: bool,
    /// Accounted retained memory for active parts and catalog entries.
    pub(crate) metadata_bytes: usize,
}

impl JournalScan {
    /// Accounted retained memory for active parts and catalog entries.
    #[must_use]
    pub const fn metadata_bytes(&self) -> usize {
        self.metadata_bytes
    }
}

/// Result of scanning a [`super::LocalDir`].
#[derive(Debug, Clone)]
pub struct LocalScan {
    /// Finished segments, sorted by numeric [`SegmentId`].
    ///
    /// The collection is shared so cloning a snapshot does not copy one entry
    /// per retained segment.
    #[expect(
        clippy::rc_buffer,
        reason = "Arc<Vec<_>> preserves the completed Vec allocation; Vec-to-Arc-slice would copy it"
    )]
    pub finished: Arc<Vec<FinalUnit>>,
    /// Valid parts from `active.wal`, in journal order.
    #[expect(
        clippy::rc_buffer,
        reason = "snapshot clones and unchanged refreshes share the validated active baseline"
    )]
    pub active: Arc<Vec<ActivePart>>,
    /// Non-fatal scan diagnostics. A successful strict owned-tree scan leaves
    /// this empty.
    pub warnings: Vec<StoreWarning>,
    /// Byte offset of the end of the last valid journal frame.
    ///
    /// This is the resumable offset for the next incremental scan and equals
    /// the complete journal length after a successful strict v1 scan.
    pub valid_len: u64,
    /// Whether the captured journal state is a committed reset marker phase.
    pub committed_reset: bool,
    /// Retained scan metadata other than the warning vector allocation.
    pub(crate) metadata_bytes: usize,
}

/// One captured, append-only prefix of `active.wal`.
///
/// The validated parts and prefix length come from one [`LocalScan`]. Later
/// appends to the same journal generation are deliberately outside this view.
#[derive(Debug, Clone)]
pub struct ActiveSnapshot {
    pub(crate) file: Arc<File>,
    #[expect(
        clippy::rc_buffer,
        reason = "the snapshot shares the exact validated LocalScan allocation"
    )]
    pub(crate) active: Arc<Vec<ActivePart>>,
    pub(crate) part_count: usize,
    pub(crate) valid_len: u64,
    pub(crate) segment_id: SegmentId,
}

/// A non-fatal storage diagnostic retained in the scan API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreWarning {
    /// Typed identity of the affected owned-store object.
    pub affected: StoreObject,
    /// Low-cardinality reason that never retains source bytes or error text.
    pub reason: StoreWarningReason,
    /// Exact filesystem identity when discovery reached a concrete inode.
    pub identity: Option<FileIdentity>,
    /// Bounded OS failure details for a localized I/O warning.
    pub failure: Option<StoreIoFailure>,
}

/// An owned-store object affected by a non-fatal diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreObject {
    /// One canonical finished segment address.
    Segment(SegmentAddress),
    /// The root-level `active.wal` journal.
    ActiveJournal,
    /// One unsupported tree entry that discovery did not follow or interpret.
    Foreign(PathIdentity),
}

/// Low-cardinality reason for a non-fatal store diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreWarningReason {
    /// A stable finished segment failed complete format validation and was excluded.
    InvalidZms(InvalidZmsReason),
    /// The live journal could not be admitted; finished discovery continued.
    ActiveJournal(ActiveJournalWarningReason),
    /// One tree entry was excluded from the supported owned-store grammar.
    ForeignEntry(ForeignEntryReason),
}

impl StoreWarningReason {
    /// Stable low-cardinality code suitable for logs and metric labels.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidZms(reason) => reason.code(),
            Self::ActiveJournal(ActiveJournalWarningReason::Corrupt) => "active_journal_corrupt",
            Self::ActiveJournal(ActiveJournalWarningReason::Io) => "active_journal_io",
            Self::ForeignEntry(ForeignEntryReason::UnsupportedName) => {
                "foreign_entry_unsupported_name"
            }
            Self::ForeignEntry(ForeignEntryReason::UnsupportedType) => {
                "foreign_entry_unsupported_type"
            }
            Self::ForeignEntry(ForeignEntryReason::SymbolicLink) => "foreign_entry_symbolic_link",
            Self::ForeignEntry(ForeignEntryReason::UnsupportedFlatArtifact) => {
                "foreign_entry_unsupported_flat_artifact"
            }
            Self::ForeignEntry(ForeignEntryReason::NonAsciiName) => "foreign_entry_non_ascii_name",
            Self::ForeignEntry(ForeignEntryReason::MisbucketedSegment) => {
                "foreign_entry_misbucketed_segment"
            }
        }
    }
}

/// Why a strict live-journal scan degraded to finished-only discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveJournalWarningReason {
    /// Journal framing or committed bytes were structurally invalid.
    Corrupt,
    /// A localized journal open or read operation failed.
    Io,
}

/// Stable format failure observed for a finished segment.
///
/// Variants deliberately omit corrupt bytes, raw decoder messages, untrusted
/// numeric fields, checksums, and paths so callers can log or count them
/// without leaking telemetry or creating unbounded metric labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvalidZmsReason {
    /// The file is too short to contain the required framing.
    TooSmall,
    /// The opening ZMS magic is invalid.
    BadMagic,
    /// The trailing tail index is malformed.
    TailIndex,
    /// The catalog uses an unsupported container version.
    UnsupportedFormat,
    /// The tail index declares an impossible or excessive catalog length.
    BadCatalogLength,
    /// The catalog framing, entry count, or checksum is invalid.
    Catalog,
    /// Catalog entries do not describe canonical section geometry.
    CanonicalLayout,
    /// At least one complete section body has a mismatched CRC32C.
    SectionChecksum,
    /// A positional read reached stable end-of-file before required bytes.
    Truncated,
    /// A localized file open, read, or metadata operation failed.
    Io,
}

/// Stage of a localized, non-fatal store I/O failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreIoOperation {
    /// Opening one already-discovered object.
    Open,
    /// Reading format bytes from one opened object.
    Read,
    /// Revalidating one opened object's filesystem identity.
    Metadata,
}

/// Bounded, payload-free details for a localized store I/O failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreIoFailure {
    /// Operation that failed.
    pub operation: StoreIoOperation,
    /// Portable I/O error category.
    pub error_kind: std::io::ErrorKind,
    /// Platform error number, when available.
    pub raw_os_error: Option<i32>,
}

impl StoreIoFailure {
    pub(crate) fn from_error(operation: StoreIoOperation, error: &std::io::Error) -> Self {
        Self {
            operation,
            error_kind: error.kind(),
            raw_os_error: error.raw_os_error(),
        }
    }
}

impl InvalidZmsReason {
    /// Stable low-cardinality code suitable for logs and metric labels.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooSmall => "invalid_zms_too_small",
            Self::BadMagic => "invalid_zms_bad_magic",
            Self::TailIndex => "invalid_zms_tail_index",
            Self::UnsupportedFormat => "invalid_zms_unsupported_format",
            Self::BadCatalogLength => "invalid_zms_bad_catalog_length",
            Self::Catalog => "invalid_zms_catalog",
            Self::CanonicalLayout => "invalid_zms_canonical_layout",
            Self::SectionChecksum => "invalid_zms_section_checksum",
            Self::Truncated => "invalid_zms_truncated",
            Self::Io => "invalid_zms_io",
        }
    }
}

/// Why a storage read failed.
#[derive(Debug)]
pub enum StoreError {
    /// An I/O error occurred while reading the file.
    Io(std::io::Error),
    /// The typed data layout rejected an unsafe or malformed tree.
    Layout(LayoutError),
    /// A journal part declares a body larger than the reader accepts.
    ActivePartTooLarge {
        /// Claimed active part body size, bytes.
        len: usize,
        /// Maximum accepted active part body size, bytes.
        max: u64,
    },
    /// The source is too short to contain a tail index.
    TooSmall,
    /// The first four bytes are not the ZMS magic.
    BadMagic,
    /// The trailing ZMS tail index failed to decode.
    TailIndex(kronika_format::DecodeError),
    /// The catalog declares a format version this build does not support.
    UnsupportedFormat {
        /// The `format_version` found in the catalog.
        version: u32,
    },
    /// `catalog_len` does not fit between the magic and the tail index.
    BadCatalogLen,
    /// The catalog bytes failed to decode.
    Catalog(kronika_format::DecodeError),
    /// The catalog does not describe the canonical physical section layout.
    SectionLayout(kronika_format::CatalogLayoutError),
    /// A catalog entry points outside the section area.
    OutOfBounds,
    /// A complete section body does not match its catalog checksum.
    SectionChecksum {
        /// Section type whose CRC32C did not match.
        type_id: u32,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Layout(err) => write!(f, "data layout: {err}"),
            Self::ActivePartTooLarge { len, max } => {
                write!(
                    f,
                    "active part of {len} bytes exceeds the part limit of {max}"
                )
            }
            Self::TooSmall => write!(f, "source is too small for a ZMS segment"),
            Self::BadMagic => write!(f, "source does not start with ZMS1 magic"),
            Self::TailIndex(err) => write!(f, "tail index decode failed: {err}"),
            Self::UnsupportedFormat { version } => {
                write!(f, "unsupported format version {version}")
            }
            Self::BadCatalogLen => write!(f, "catalog_len does not fit in the source"),
            Self::Catalog(err) => write!(f, "catalog decode failed: {err}"),
            Self::SectionLayout(err) => write!(f, "invalid section layout: {err}"),
            Self::OutOfBounds => write!(f, "a catalog entry points outside the section area"),
            Self::SectionChecksum { type_id } => {
                write!(
                    f,
                    "section {type_id} body checksum does not match its catalog"
                )
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::SectionLayout(err) => Some(err),
            Self::Catalog(err) | Self::TailIndex(err) => Some(err),
            Self::Layout(err) => Some(err),
            Self::ActivePartTooLarge { .. }
            | Self::TooSmall
            | Self::BadMagic
            | Self::UnsupportedFormat { .. }
            | Self::BadCatalogLen
            | Self::OutOfBounds
            | Self::SectionChecksum { .. } => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<LayoutError> for StoreError {
    fn from(err: LayoutError) -> Self {
        Self::Layout(err)
    }
}
