//! What can go wrong encoding or decoding one section body.

use super::{Error, fmt};

/// Why a section failed to encode or decode.
#[derive(Debug)]
pub enum CodecError {
    /// An Arrow operation failed (building the record batch).
    Arrow(arrow_schema::ArrowError),
    /// A Parquet operation failed (writing or reading the file).
    Parquet(parquet::errors::ParquetError),
    /// More rows than [`MAX_SECTION_ROWS`] were given to encode, or a
    /// section claims or holds more on decode.
    TooManyRows {
        /// The row count that exceeded the cap.
        rows: usize,
        /// The enforced cap.
        max: usize,
    },
    /// Parquet metadata reports a negative or unrepresentable row count.
    InvalidRowCount {
        /// The raw `num_rows` from Parquet metadata.
        raw: i64,
    },
    /// Parquet metadata disagrees with the row count in the segment catalog.
    RowCountMismatch {
        /// Rows declared by the segment catalog.
        expected: u64,
        /// Rows declared by Parquet metadata.
        got: u64,
    },
    /// The section byte length is above [`MAX_SECTION_BYTES`].
    SectionTooLarge {
        /// The byte length that exceeded the cap.
        len: usize,
        /// The enforced cap.
        max: usize,
    },
    /// Parquet metadata declares more decoded column bytes than the work cap.
    DecodedSectionTooLarge {
        /// Aggregate uncompressed column bytes.
        len: usize,
        /// The enforced cap.
        max: usize,
    },
    /// The section has more than [`MAX_ROW_GROUPS`] row groups.
    TooManyRowGroups {
        /// The row-group count that exceeded the cap.
        groups: usize,
        /// The enforced cap.
        max: usize,
    },
    /// Parquet footer, column ranges, or page headers are inconsistent.
    InvalidPageLayout,
    /// A variable-width dictionary section uses Parquet dictionary encoding,
    /// whose index expansion is not bounded by encoded page sizes.
    DictionaryEncodingUnsupported,
    /// A page declares an encoding outside the admitted profile; delta and
    /// stream-split encodings materialize more bytes than the pages declare.
    UnsupportedPageEncoding {
        /// The raw Parquet encoding id.
        encoding: i32,
    },
    /// A column required by the contract is absent from the decoded file.
    MissingColumn {
        /// The missing column name.
        name: &'static str,
    },
    /// A decoded column has a different Arrow type than the contract.
    ColumnType {
        /// The column name.
        name: &'static str,
    },
    /// A `NULL` appeared in a column the contract declares non-nullable.
    ///
    /// Required columns must not decode a missing value as zero.
    NullInRequiredColumn {
        /// The column name.
        name: &'static str,
    },
    /// A `List<Int32>` column holds more child values than the codec accepts.
    TooManyListValues {
        /// The column name.
        name: &'static str,
        /// The child value count that exceeded the cap.
        values: usize,
        /// The enforced cap.
        max: usize,
    },
    /// No registered type has the requested `type_id`.
    UnknownType {
        /// The unrecognized id.
        type_id: u32,
    },
    /// A decoded section's schema does not match the contract it was decoded
    /// against (column set, order, types, or nullability).
    SchemaMismatch,
    /// A section's computed CRC does not match the catalog's, so the bytes are
    /// corrupt (or not the section the catalog points at).
    SectionCrcMismatch {
        /// The CRC the catalog claims.
        expected: u32,
        /// The CRC computed over the bytes.
        got: u32,
    },
    /// A decode failed for a known `type_id`.
    Section {
        /// The section's `type_id`.
        type_id: u32,
        /// Input section bytes.
        bytes_in: usize,
        /// The underlying decode error.
        source: Box<Self>,
    },
}

impl CodecError {
    /// The section `type_id` this error is about, if known.
    #[must_use]
    pub const fn section_type_id(&self) -> Option<u32> {
        match self {
            Self::UnknownType { type_id } | Self::Section { type_id, .. } => Some(*type_id),
            // Add new type-tagged variants here so failure metrics keep their
            // `{type_id}` label.
            _ => None,
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arrow(err) => write!(f, "arrow: {err}"),
            Self::Parquet(err) => write!(f, "parquet: {err}"),
            Self::TooManyRows { rows, max } => {
                write!(f, "section has {rows} rows, above the cap of {max}")
            }
            Self::InvalidRowCount { raw } => {
                write!(f, "section claims an invalid row count of {raw}")
            }
            Self::RowCountMismatch { expected, got } => {
                write!(
                    f,
                    "section catalog declares {expected} rows but Parquet declares {got}"
                )
            }
            Self::SectionTooLarge { len, max } => {
                write!(f, "section is {len} bytes, above the cap of {max}")
            }
            Self::DecodedSectionTooLarge { len, max } => write!(
                f,
                "section declares {len} decoded bytes, above the work cap of {max}"
            ),
            Self::TooManyRowGroups { groups, max } => {
                write!(f, "section has {groups} row groups, above the cap of {max}")
            }
            Self::InvalidPageLayout => {
                f.write_str("Parquet page layout violates the bounded footer contract")
            }
            Self::DictionaryEncodingUnsupported => {
                f.write_str("Parquet dictionary encoding is not admitted for dictionary sections")
            }
            Self::UnsupportedPageEncoding { encoding } => {
                write!(f, "Parquet page encoding {encoding} is outside the profile")
            }
            Self::MissingColumn { name } => write!(f, "decoded section lacks column {name:?}"),
            Self::ColumnType { name } => write!(f, "decoded column {name:?} has the wrong type"),
            Self::NullInRequiredColumn { name } => {
                write!(
                    f,
                    "decoded column {name:?} has a NULL but the contract forbids it"
                )
            }
            Self::TooManyListValues { name, values, max } => {
                write!(
                    f,
                    "List<Int32> column {name:?} has {values} child values, above the cap of {max}"
                )
            }
            Self::UnknownType { type_id } => write!(f, "no registered type has id {type_id}"),
            Self::SchemaMismatch => {
                write!(f, "decoded section schema does not match the contract")
            }
            Self::SectionCrcMismatch { expected, got } => {
                write!(
                    f,
                    "section CRC {got:#010x} does not match the catalog's {expected:#010x}"
                )
            }
            Self::Section {
                type_id,
                bytes_in,
                source,
            } => write!(f, "decoding type {type_id} ({bytes_in} bytes): {source}"),
        }
    }
}

impl Error for CodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Arrow(err) => Some(err),
            Self::Parquet(err) => Some(err),
            Self::TooManyRows { .. }
            | Self::InvalidRowCount { .. }
            | Self::RowCountMismatch { .. }
            | Self::SectionTooLarge { .. }
            | Self::DecodedSectionTooLarge { .. }
            | Self::TooManyRowGroups { .. }
            | Self::InvalidPageLayout
            | Self::DictionaryEncodingUnsupported
            | Self::UnsupportedPageEncoding { .. }
            | Self::MissingColumn { .. }
            | Self::ColumnType { .. }
            | Self::NullInRequiredColumn { .. }
            | Self::TooManyListValues { .. }
            | Self::UnknownType { .. }
            | Self::SchemaMismatch
            | Self::SectionCrcMismatch { .. } => None,
            Self::Section { source, .. } => Some(source.as_ref()),
        }
    }
}

impl From<arrow_schema::ArrowError> for CodecError {
    fn from(err: arrow_schema::ArrowError) -> Self {
        Self::Arrow(err)
    }
}

impl From<parquet::errors::ParquetError> for CodecError {
    fn from(err: parquet::errors::ParquetError) -> Self {
        Self::Parquet(err)
    }
}
