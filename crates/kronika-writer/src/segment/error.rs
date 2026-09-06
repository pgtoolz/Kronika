//! What can go wrong while turning a journal into a segment.

use super::{CodecError, Error, JournalError, LayoutError, PartError, SegmentId, fmt, io};

/// Why writing a segment failed.
#[derive(Debug)]
pub enum WriteError {
    /// A filesystem operation failed.
    Io(io::Error),
    /// The typed data layout rejected publication.
    Layout(LayoutError),
    /// Reading a part back from the journal failed.
    Journal(JournalError),
    /// A journal part did not validate as a ZMS container.
    Part(PartError),
    /// A registered or dictionary Parquet section was invalid.
    Codec(CodecError),
    /// The journal holds no parts, so there is nothing to write.
    Empty,
    /// The journal and requested destination carry different identities.
    SegmentIdMismatch {
        /// Identity stored in the journal.
        journal: SegmentId,
        /// Requested final address.
        destination: SegmentId,
    },
    /// The writer produced a ZMS that failed its own structural checks.
    GeneratedSegmentInvalid,
    /// An existing final ZMS at the recovered identity is structurally invalid.
    ExistingSegmentInvalid,
    /// An existing valid ZMS differs from the journal's deterministic result.
    ExistingSegmentMismatch,
    /// The combined section catalog exceeds the writer's fixed admission limit.
    CatalogTooLarge {
        /// Number of entries the next journal part would produce.
        attempted_entries: usize,
        /// Maximum supported entries in one segment.
        max_entries: usize,
    },
    /// Reserving bounded memory for the combined section catalog failed.
    CatalogAllocation(std::collections::TryReserveError),
    /// A journal part uses a different internal format version.
    UnsupportedFormat {
        /// Version read from the part catalog.
        version: u32,
    },
    /// Catalog rows and decoded rows do not agree.
    RowCountMismatch {
        /// Section type.
        type_id: u32,
        /// Rows declared by the catalog.
        declared: u32,
        /// Rows produced by Parquet decode.
        decoded: usize,
    },
    /// One dictionary id has conflicting bytes, metadata, or placement.
    DictionaryConflict {
        /// Conflicting dictionary id.
        str_id: u64,
    },
    /// The requested finished catalog has inverted timestamp bounds.
    InvalidTimestampBounds {
        /// Minimal timestamp supplied by the caller.
        min_ts: i64,
        /// Maximal timestamp supplied by the caller.
        max_ts: i64,
    },
    /// A checked size, count, or offset calculation overflowed.
    ArithmeticOverflow {
        /// Quantity that overflowed.
        what: &'static str,
    },
    /// The journal contains more section descriptors than writing will retain.
    TooManySections {
        /// Descriptor count encountered.
        sections: usize,
        /// Enforced bound.
        max: usize,
    },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "segment io: {err}"),
            Self::Layout(err) => write!(f, "segment layout: {err}"),
            Self::Journal(err) => write!(f, "reading a journal part: {err}"),
            Self::Part(err) => write!(f, "invalid journal part: {err}"),
            Self::Codec(err) => write!(f, "invalid section: {err}"),
            Self::Empty => write!(f, "the journal holds no parts to write"),
            Self::SegmentIdMismatch {
                journal,
                destination,
            } => write!(
                f,
                "journal segment id {journal} does not match destination {destination}"
            ),
            Self::GeneratedSegmentInvalid => {
                f.write_str("the generated segment failed structural validation")
            }
            Self::ExistingSegmentInvalid => {
                f.write_str("the existing segment failed structural validation")
            }
            Self::ExistingSegmentMismatch => {
                f.write_str("the existing segment differs from the recovered journal")
            }
            Self::CatalogTooLarge {
                attempted_entries,
                max_entries,
            } => write!(
                f,
                "segment catalog would contain {attempted_entries} entries, limit is {max_entries}"
            ),
            Self::CatalogAllocation(error) => {
                write!(f, "reserving the bounded segment catalog failed: {error}")
            }
            Self::UnsupportedFormat { version } => {
                write!(f, "journal part uses unsupported format version {version}")
            }
            Self::RowCountMismatch {
                type_id,
                declared,
                decoded,
            } => write!(
                f,
                "section {type_id} declares {declared} rows but decodes {decoded}"
            ),
            Self::DictionaryConflict { str_id } => {
                write!(f, "dictionary id {str_id} has conflicting representations")
            }
            Self::InvalidTimestampBounds { min_ts, max_ts } => write!(
                f,
                "segment timestamp bounds are inverted: {min_ts} is after {max_ts}"
            ),
            Self::ArithmeticOverflow { what } => write!(f, "{what} overflow"),
            Self::TooManySections { sections, max } => {
                write!(
                    f,
                    "journal has {sections} sections, above the limit of {max}"
                )
            }
        }
    }
}

impl Error for WriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Layout(err) => Some(err),
            Self::Journal(err) => Some(err),
            Self::Part(err) => Some(err),
            Self::Codec(err) => Some(err),
            Self::CatalogAllocation(error) => Some(error),
            Self::Empty
            | Self::SegmentIdMismatch { .. }
            | Self::GeneratedSegmentInvalid
            | Self::ExistingSegmentInvalid
            | Self::ExistingSegmentMismatch
            | Self::CatalogTooLarge { .. }
            | Self::UnsupportedFormat { .. }
            | Self::RowCountMismatch { .. }
            | Self::DictionaryConflict { .. }
            | Self::InvalidTimestampBounds { .. }
            | Self::ArithmeticOverflow { .. }
            | Self::TooManySections { .. } => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<JournalError> for WriteError {
    fn from(err: JournalError) -> Self {
        Self::Journal(err)
    }
}

impl From<CodecError> for WriteError {
    fn from(err: CodecError) -> Self {
        Self::Codec(err)
    }
}

impl From<parquet::errors::ParquetError> for WriteError {
    fn from(err: parquet::errors::ParquetError) -> Self {
        Self::Codec(CodecError::Parquet(err))
    }
}

impl From<arrow_schema::ArrowError> for WriteError {
    fn from(err: arrow_schema::ArrowError) -> Self {
        Self::Codec(CodecError::Arrow(err))
    }
}

impl From<LayoutError> for WriteError {
    fn from(err: LayoutError) -> Self {
        Self::Layout(err)
    }
}
