//! What can go wrong while appending to or opening the journal.

use super::{Error, JournalHeaderError, JournalScanError, LayoutError, PartError, SegmentId, fmt};

/// Error returned by a journal operation.
#[derive(Debug)]
pub enum JournalError {
    /// The underlying file operation failed.
    Io(std::io::Error),
    /// The data-root capability rejected an unsafe or inaccessible object.
    Layout(LayoutError),
    /// The configured file cap is outside the version-1 admission range.
    InvalidMaxJournalLen {
        /// Configured cap.
        value: usize,
        /// Smallest accepted cap.
        minimum: usize,
        /// Largest accepted cap.
        maximum: usize,
    },
    /// The configured part-count cap is outside the version-1 admission range.
    InvalidMaxParts {
        /// Configured cap.
        value: usize,
        /// Smallest accepted cap.
        minimum: usize,
        /// Largest accepted cap.
        maximum: usize,
    },
    /// The configured part-body cap is outside the version-1 admission range.
    InvalidMaxPartLen {
        /// Configured cap.
        value: u64,
        /// Smallest accepted cap.
        minimum: u64,
        /// Largest accepted cap.
        maximum: u64,
    },
    /// An existing canonical journal exceeds its configured file cap.
    JournalTooLarge {
        /// Existing file length.
        len: u64,
        /// Configured cap.
        max: usize,
    },
    /// An existing or appended frame would exceed the configured part-count cap.
    TooManyParts {
        /// Configured cap.
        max: usize,
    },
    /// A foreign or differently versioned journal header was found.
    UnsupportedJournalFormat,
    /// The file ends before the complete version-1 header.
    TornHeader {
        /// Actual file length.
        len: u64,
    },
    /// The complete header failed a state or checksum check.
    InvalidHeader(JournalHeaderError),
    /// The header's recorded frame length differs from the physical tail.
    BodyLengthMismatch {
        /// Length recorded in the header.
        recorded: u64,
        /// Physical bytes after the header.
        actual: u64,
    },
    /// An empty header has trailing frame bytes.
    EmptyWithFrames {
        /// Physical frame-tail length.
        body_len: u64,
    },
    /// An active header has no complete first frame.
    ActiveWithoutFirstFrame,
    /// A version-1 body contains torn or damaged frame bytes.
    DamagedBody,
    /// The stored raw identity cannot be represented by the calendar layout.
    InvalidSegmentId(LayoutError),
    /// An append attempted to mix two segment identities.
    SegmentIdMismatch {
        /// Identity persisted in the journal.
        expected: SegmentId,
        /// Identity supplied by the append.
        got: SegmentId,
    },
    /// The part is larger than the configured frame limit.
    PartTooLarge {
        /// Length of the rejected part, bytes.
        len: usize,
        /// The configured limit, bytes.
        max: u64,
    },
    /// Appending would grow the journal past its configured cap.
    Full {
        /// Current journal length, bytes.
        len: usize,
        /// Configured cap, bytes.
        max: usize,
    },
    /// The part is not a valid ZMS part.
    InvalidPart(PartError),
    /// The reference no longer points inside the current journal generation.
    StalePartRef {
        /// Offset of the rejected reference.
        offset: usize,
        /// Length of the rejected reference.
        len: usize,
    },
    /// A failed write could not be rolled back to the preceding durable state.
    RollbackFailed {
        /// Error from the original write or synchronization.
        operation: std::io::Error,
        /// Error encountered while restoring the old file.
        rollback: std::io::Error,
    },
    /// A committed reset marker could not be reduced to the canonical empty file.
    ResetIncomplete(std::io::Error),
    /// A previous operation left the open journal in an indeterminate state.
    Poisoned,
    /// A writer-owned rotated generation is not the canonical empty bytes.
    FreshGenerationInvalid {
        /// Observed file length.
        len: u64,
    },
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "journal I/O: {error}"),
            Self::Layout(error) => write!(f, "journal layout: {error}"),
            Self::InvalidMaxJournalLen {
                value,
                minimum,
                maximum,
            } => write!(
                f,
                "journal length cap {value} is outside {minimum}..={maximum}"
            ),
            Self::InvalidMaxParts {
                value,
                minimum,
                maximum,
            } => write!(
                f,
                "journal part-count cap {value} is outside {minimum}..={maximum}"
            ),
            Self::InvalidMaxPartLen {
                value,
                minimum,
                maximum,
            } => write!(
                f,
                "journal part-body cap {value} is outside {minimum}..={maximum} bytes"
            ),
            Self::JournalTooLarge { len, max } => {
                write!(
                    f,
                    "active.wal length {len} exceeds the configured cap of {max}"
                )
            }
            Self::TooManyParts { max } => {
                write!(f, "active.wal exceeds the configured cap of {max} parts")
            }
            Self::UnsupportedJournalFormat => f.write_str("active.wal is not a version-1 journal"),
            Self::TornHeader { len } => {
                write!(f, "active.wal has a torn header of {len} bytes")
            }
            Self::InvalidHeader(error) => write!(f, "invalid active.wal header: {error}"),
            Self::BodyLengthMismatch { recorded, actual } => write!(
                f,
                "active.wal header records {recorded} body bytes, but the file contains {actual}"
            ),
            Self::EmptyWithFrames { body_len } => {
                write!(f, "empty active.wal header has {body_len} trailing bytes")
            }
            Self::ActiveWithoutFirstFrame => {
                f.write_str("active active.wal has no complete first frame")
            }
            Self::DamagedBody => f.write_str("active.wal contains torn or damaged frames"),
            Self::InvalidSegmentId(error) => {
                write!(f, "active.wal stores an invalid segment id: {error}")
            }
            Self::SegmentIdMismatch { expected, got } => write!(
                f,
                "active.wal belongs to segment {expected}, not supplied segment {got}"
            ),
            Self::PartTooLarge { len, max } => {
                write!(f, "part of {len} bytes exceeds the frame limit of {max}")
            }
            Self::Full { len, max } => write!(
                f,
                "journal of {len} bytes would exceed the cap of {max}; write and reset first"
            ),
            Self::InvalidPart(error) => write!(f, "part is not a valid ZMS part: {error}"),
            Self::StalePartRef { offset, len } => {
                write!(f, "part reference {offset}+{len} is outside active.wal")
            }
            Self::RollbackFailed {
                operation,
                rollback,
            } => write!(
                f,
                "journal write failed ({operation}) and rollback also failed ({rollback})"
            ),
            Self::ResetIncomplete(error) => write!(
                f,
                "journal reset was committed but requires restart recovery: {error}"
            ),
            Self::Poisoned => {
                f.write_str("journal is poisoned after an incomplete persistence operation")
            }
            Self::FreshGenerationInvalid { len } => write!(
                f,
                "fresh rotated journal generation is not the canonical empty file ({len} bytes)"
            ),
        }
    }
}

impl Error for JournalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) | Self::ResetIncomplete(error) => Some(error),
            Self::Layout(error) | Self::InvalidSegmentId(error) => Some(error),
            Self::InvalidHeader(error) => Some(error),
            Self::InvalidPart(error) => Some(error),
            Self::RollbackFailed { operation, .. } => Some(operation),
            Self::UnsupportedJournalFormat
            | Self::InvalidMaxJournalLen { .. }
            | Self::InvalidMaxParts { .. }
            | Self::InvalidMaxPartLen { .. }
            | Self::JournalTooLarge { .. }
            | Self::TooManyParts { .. }
            | Self::TornHeader { .. }
            | Self::BodyLengthMismatch { .. }
            | Self::EmptyWithFrames { .. }
            | Self::ActiveWithoutFirstFrame
            | Self::DamagedBody { .. }
            | Self::SegmentIdMismatch { .. }
            | Self::PartTooLarge { .. }
            | Self::Full { .. }
            | Self::StalePartRef { .. }
            | Self::Poisoned
            | Self::FreshGenerationInvalid { .. } => None,
        }
    }
}

impl From<std::io::Error> for JournalError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<LayoutError> for JournalError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

pub(super) fn map_scan_error(error: JournalScanError) -> JournalError {
    match error {
        JournalScanError::Io(error) => JournalError::Io(error),
        JournalScanError::PartLimitExceeded { limit } => JournalError::TooManyParts { max: limit },
    }
}
