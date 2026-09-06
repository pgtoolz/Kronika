//! Vocabulary the strict scan produces: what was found and where.

use std::ffi::CString;
use std::fmt;

use crate::time::{SegmentAddress, UtcDay};

use super::FileIdentity;
use super::names::hash_name;

/// Final data kind addressed inside a UTC day directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Immutable source segment.
    Zms,
    /// Replaceable derived index sidecar.
    Idx,
}

/// Recognized crash-remnant or in-progress temporary file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryKind {
    /// Writer ZMS publication.
    Zms,
    /// Index sidecar publication.
    Idx,
    /// Index writeability probe.
    IndexProbe,
}

/// A verified temporary object returned by strict discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporaryObject {
    /// Segment address encoded by the file and parent day.
    pub address: SegmentAddress,
    /// Publisher that owns cleanup of this object.
    pub kind: TemporaryKind,
    /// No-follow identity observed by discovery.
    pub identity: FileIdentity,
    pub(super) file_name: String,
}

impl TemporaryObject {
    /// Returns the verified leaf file name.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }
}

/// The verified parent scope of an entry that was not interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryScope {
    /// Direct child of the data root.
    Root,
    /// Direct child of a valid UTC year directory.
    Year {
        /// Four-digit UTC year.
        year: u16,
    },
    /// Direct child of a valid UTC month directory.
    Month {
        /// Four-digit UTC year.
        year: u16,
        /// UTC month, `1..=12`.
        month: u8,
    },
    /// Direct child of a valid UTC day directory.
    Day(UtcDay),
}

/// Filesystem type observed without following a symbolic link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryFileType {
    /// Regular file.
    RegularFile,
    /// Directory.
    Directory,
    /// Symbolic link. Its target was not inspected.
    Symlink,
    /// FIFO, socket, device, or another unsupported object.
    Other,
}

/// Bounded, payload-free identity for one named tree entry.
///
/// The raw name is deliberately not exposed. `name_hash` is a stable,
/// non-cryptographic correlation token, not a content digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathIdentity {
    /// Verified parent scope.
    pub scope: EntryScope,
    /// Stable hash of the raw leaf-name bytes.
    pub name_hash: u64,
    /// Raw leaf-name length in bytes.
    pub name_len: u16,
    /// No-follow filesystem type.
    pub file_type: EntryFileType,
    /// No-follow filesystem identity and metadata.
    pub file: FileIdentity,
}

/// Why a tree entry was excluded from the supported readable grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForeignEntryReason {
    /// The name is outside the grammar for its verified parent.
    UnsupportedName,
    /// The name is reserved, but its filesystem type is unsupported.
    UnsupportedType,
    /// A symbolic link was encountered and not followed.
    SymbolicLink,
    /// A root-level ZMS or IDX belongs to the unsupported flat layout.
    UnsupportedFlatArtifact,
    /// Raw name bytes are not ASCII and were not interpreted.
    NonAsciiName,
    /// A segment-like leaf points to a different UTC bucket.
    MisbucketedSegment,
}

/// Typed diagnostic for one locally excluded tree entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryDiagnostic {
    /// Bounded path/object identity.
    pub path: PathIdentity,
    /// Reason the entry was not interpreted.
    pub reason: ForeignEntryReason,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct EntryPath {
    pub(super) parent: EntryParent,
    pub(super) name: CString,
}

impl fmt::Debug for EntryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EntryPath")
            .field("scope", &self.parent.scope())
            .field("name_hash", &hash_name(self.name.as_bytes()))
            .field("name_len", &self.name.as_bytes().len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryParent {
    Root,
    Year(u16),
    Month { year: u16, month: u8 },
    Day(UtcDay),
}

impl EntryParent {
    pub(super) const fn scope(self) -> EntryScope {
        match self {
            Self::Root => EntryScope::Root,
            Self::Year(year) => EntryScope::Year { year },
            Self::Month { year, month } => EntryScope::Month { year, month },
            Self::Day(day) => EntryScope::Day(day),
        }
    }
}

/// Opaque capability for one unsupported entry found by a tolerant scan.
///
/// Its raw name remains private so only [`WriterOwner`] can mutate it through
/// descriptor-relative, identity-checked operations.
#[derive(Clone, PartialEq, Eq)]
pub struct ForeignEntry {
    pub(super) diagnostic: EntryDiagnostic,
    pub(super) path: EntryPath,
}

impl ForeignEntry {
    /// Returns the bounded diagnostic without exposing the raw leaf name.
    #[must_use]
    pub const fn diagnostic(&self) -> EntryDiagnostic {
        self.diagnostic
    }
}

impl fmt::Debug for ForeignEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForeignEntry")
            .field("diagnostic", &self.diagnostic)
            .finish_non_exhaustive()
    }
}

/// Discovered final artifacts for one source segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentArtifacts {
    /// Logical and physical segment address.
    pub address: SegmentAddress,
    /// Filesystem identity observed for the immutable ZMS name.
    pub zms_identity: FileIdentity,
    /// Current ZMS file size.
    pub zms_bytes: u64,
    /// Current sibling IDX file size, when present.
    pub idx_bytes: Option<u64>,
}

/// Bytes reclaimed by removing one finished segment during rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SegmentRemoval {
    /// Bytes freed by unlinking the immutable ZMS; zero if it was already gone.
    pub zms_bytes: u64,
    /// Bytes freed by unlinking the sibling IDX, when one was present.
    pub idx_bytes: Option<u64>,
}

impl SegmentRemoval {
    /// Total bytes reclaimed across the ZMS and its sibling IDX.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        match self.idx_bytes {
            Some(idx) => self.zms_bytes.saturating_add(idx),
            None => self.zms_bytes,
        }
    }
}

/// Complete result of one successful strict, bounded traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSnapshot {
    /// Every valid UTC day directory visited, including empty days.
    pub days: Vec<UtcDay>,
    /// Finished ZMS segments in numeric `SegmentId` order.
    pub segments: Vec<SegmentArtifacts>,
    /// IDX files without a sibling ZMS, in numeric order.
    pub orphan_indexs: Vec<SegmentAddress>,
    /// Recognized temporary files.
    pub temporaries: Vec<TemporaryObject>,
    /// Unsupported entries excluded locally without aborting valid discovery.
    pub foreign_entries: Vec<ForeignEntry>,
    /// Number of filesystem entries visited.
    pub visited_entries: usize,
    /// Accounted metadata bytes.
    pub metadata_bytes: usize,
}

/// Byte occupancy of the filesystem that backs one data root.
///
/// Both figures come from a single `statvfs` of the root descriptor and count
/// every user of the partition, not only `Kronika` data. `used_bytes` is the
/// classic "how full is the filesystem" figure (`total − free`), the basis for
/// the `auto:P` retention target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_field_names,
    reason = "the shared `_bytes` suffix is the documented unit of every counter"
)]
pub struct FilesystemUsage {
    /// Total addressable bytes of the partition.
    pub total_bytes: u64,
    /// Bytes occupied by all data on the partition.
    pub used_bytes: u64,
    /// Bytes a non-privileged writer may still allocate (`f_bavail`).
    pub available_bytes: u64,
}
