use std::error::Error;
use std::fmt;
use std::io;

use crate::{SegmentId, UtcDay};

/// A bounded traversal resource whose configured limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// All visited directory entries.
    VisitedEntries,
    /// Entries in one UTC day directory.
    EntriesPerDay,
    /// Finished ZMS segments returned to the caller.
    Segments,
    /// Memory reserved for metadata derived from directory entries.
    MetadataBytes,
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VisitedEntries => f.write_str("visited entries"),
            Self::EntriesPerDay => f.write_str("entries per day"),
            Self::Segments => f.write_str("returned segments"),
            Self::MetadataBytes => f.write_str("metadata bytes"),
        }
    }
}

/// Process-wide mutation ownership represented by a data-root lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    /// The sole collector that may mutate the journal or publish ZMS files.
    Writer,
    /// The sole web process that may publish or remove derived index files.
    Index,
}

impl fmt::Display for OwnerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Writer => f.write_str("writer"),
            Self::Index => f.write_str("index"),
        }
    }
}

/// A structural, ownership, validation, or filesystem failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum LayoutError {
    /// An operating-system operation failed.
    Io(io::Error),
    /// A traversal limit is zero or exceeds its hard maximum.
    InvalidLimits {
        /// The invalid limit.
        kind: LimitKind,
        /// Configured value.
        value: usize,
        /// Largest accepted value.
        hard_max: usize,
    },
    /// The timestamp cannot be represented by the `0000` through `9999`
    /// calendar-directory contract.
    SegmentIdOutOfRange(i64),
    /// A ZMS or IDX appears directly under the data root.
    UnsupportedFlatLayout {
        /// Rejected root entry.
        name: String,
    },
    /// A symbolic link was encountered at a root, calendar, or file component.
    SymlinkNotAllowed {
        /// Relative diagnostic name.
        name: String,
    },
    /// A root entry is not part of the closed layout grammar.
    UnexpectedRootEntry {
        /// Rejected root entry.
        name: String,
    },
    /// A known root entry has the wrong filesystem type.
    UnexpectedRootEntryType {
        /// Rejected root entry.
        name: String,
    },
    /// A year-directory name is not exactly four ASCII digits.
    MalformedYearDirectory {
        /// Rejected entry.
        name: String,
    },
    /// A month-directory name is not `01` through `12`.
    MalformedMonthDirectory {
        /// Rejected entry.
        name: String,
    },
    /// A day-directory name is not valid for its year and month.
    MalformedDayDirectory {
        /// Rejected entry.
        name: String,
    },
    /// A calendar component is not a real directory.
    UnexpectedCalendarEntryType {
        /// Rejected relative entry.
        name: String,
    },
    /// A day-directory entry is not an accepted final or temporary file name.
    UnexpectedLeafEntry {
        /// Rejected relative entry.
        name: String,
    },
    /// A recognized leaf entry is not a regular file.
    UnexpectedLeafEntryType {
        /// Rejected relative entry.
        name: String,
    },
    /// A file name's segment timestamp points to another UTC day.
    MisbucketedSegment {
        /// Parsed segment identity.
        id: SegmentId,
        /// Day encoded by the parent directories.
        actual: UtcDay,
        /// Day derived from the segment identity.
        expected: UtcDay,
    },
    /// A bounded scan would exceed one of its resource limits.
    TraversalLimitExceeded {
        /// Exhausted resource.
        kind: LimitKind,
        /// Configured limit.
        limit: usize,
    },
    /// A final ZMS already exists and is never overwritten.
    SegmentAlreadyExists {
        /// Conflicting identity.
        id: SegmentId,
    },
    /// A publication temporary no longer names the file opened by its owner.
    TemporaryChanged {
        /// Replaced temporary leaf name.
        name: String,
    },
    /// Another process owns the requested mutation role.
    OwnerContended {
        /// Contended role.
        owner: OwnerKind,
    },
    /// The ZMS changed while a derived IDX was being built.
    SourceChanged {
        /// Changed ZMS.
        id: SegmentId,
    },
    /// No canonical active journal exists.
    ActiveJournalMissing,
    /// Every bounded root recovery-name collision slot already exists.
    RecoverySlotsExhausted {
        /// Number of attempted collision slots.
        limit: usize,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "layout I/O: {error}"),
            Self::InvalidLimits {
                kind,
                value,
                hard_max,
            } => write!(f, "invalid {kind} limit {value}; expected 1..={hard_max}"),
            Self::SegmentIdOutOfRange(value) => write!(
                f,
                "segment id {value} is outside the supported UTC year range 0000..9999"
            ),
            Self::UnsupportedFlatLayout { name } => {
                write!(
                    f,
                    "finished ZMS/IDX data-root entry `{name}` is invalid: the first supported \
                     layout is YYYY/MM/DD/N.*; the entry was not read as a segment"
                )
            }
            Self::SymlinkNotAllowed { name } => {
                write!(
                    f,
                    "symbolic links are not allowed in the data layout: `{name}`"
                )
            }
            Self::UnexpectedRootEntry { name } => {
                write!(f, "unexpected data-root entry `{name}`")
            }
            Self::UnexpectedRootEntryType { name } => {
                write!(f, "data-root entry `{name}` has an unexpected type")
            }
            Self::MalformedYearDirectory { name } => {
                write!(f, "malformed UTC year directory `{name}`")
            }
            Self::MalformedMonthDirectory { name } => {
                write!(f, "malformed UTC month directory `{name}`")
            }
            Self::MalformedDayDirectory { name } => {
                write!(f, "malformed UTC day directory `{name}`")
            }
            Self::UnexpectedCalendarEntryType { name } => {
                write!(f, "calendar entry `{name}` is not a directory")
            }
            Self::UnexpectedLeafEntry { name } => {
                write!(f, "unexpected day-directory entry `{name}`")
            }
            Self::UnexpectedLeafEntryType { name } => {
                write!(f, "day-directory entry `{name}` is not a regular file")
            }
            Self::MisbucketedSegment {
                id,
                actual,
                expected,
            } => write!(
                f,
                "segment {id} is stored under {actual}, but its UTC bucket is {expected}"
            ),
            Self::TraversalLimitExceeded { kind, limit } => {
                write!(f, "layout traversal exceeded the {kind} limit of {limit}")
            }
            Self::SegmentAlreadyExists { id } => {
                write!(f, "segment {id} already exists")
            }
            Self::TemporaryChanged { name } => {
                write!(f, "publication temporary `{name}` changed before commit")
            }
            Self::OwnerContended { owner } => {
                write!(f, "another process owns the data-root {owner} lock")
            }
            Self::SourceChanged { id } => {
                write!(f, "segment {id} changed while its index was being built")
            }
            Self::ActiveJournalMissing => f.write_str("the canonical active journal is missing"),
            Self::RecoverySlotsExhausted { limit } => {
                write!(
                    f,
                    "all {limit} bounded root recovery-name collision slots exist"
                )
            }
        }
    }
}

impl Error for LayoutError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidLimits { .. }
            | Self::SegmentIdOutOfRange(_)
            | Self::UnsupportedFlatLayout { .. }
            | Self::SymlinkNotAllowed { .. }
            | Self::UnexpectedRootEntry { .. }
            | Self::UnexpectedRootEntryType { .. }
            | Self::MalformedYearDirectory { .. }
            | Self::MalformedMonthDirectory { .. }
            | Self::MalformedDayDirectory { .. }
            | Self::UnexpectedCalendarEntryType { .. }
            | Self::UnexpectedLeafEntry { .. }
            | Self::UnexpectedLeafEntryType { .. }
            | Self::MisbucketedSegment { .. }
            | Self::TraversalLimitExceeded { .. }
            | Self::SegmentAlreadyExists { .. }
            | Self::TemporaryChanged { .. }
            | Self::OwnerContended { .. }
            | Self::SourceChanged { .. }
            | Self::ActiveJournalMissing
            | Self::RecoverySlotsExhausted { .. } => None,
        }
    }
}

impl From<io::Error> for LayoutError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
