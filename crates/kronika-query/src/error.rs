//! Transport-neutral query failures.

use std::error::Error;

use kronika_reader::ReaderError;

/// Why a recorded-data query could not be completed.
#[derive(Debug)]
#[non_exhaustive]
pub enum QueryError {
    /// No captured segment has the requested identity.
    NoSuchSegment,
    /// The selected logical section is absent.
    NoSuchSection,
    /// A requested field is absent from every compatible layout.
    NoSuchColumn(String),
    /// Requested fields do not have one compatible unit.
    MixedUnits(String),
    /// A typed filter is invalid.
    BadFilter(String),
    /// A continuation or active-prefix cursor is invalid.
    BadCursor,
    /// A row detail reference is malformed or ambiguous.
    BadLocator(String),
    /// The caller disconnected before the query completed.
    Cancelled,
    /// Captured bytes or a derived representation could not be read.
    Unreadable(Box<dyn Error + Send + Sync>),
}

impl QueryError {
    /// Reports whether a native caller can retry against a fresh capture.
    #[must_use]
    pub fn source_changed_during_read(&self) -> bool {
        let Self::Unreadable(error) = self else {
            return false;
        };
        let mut source: &(dyn Error + 'static) = error.as_ref();
        loop {
            if let Some(reader) = source.downcast_ref::<ReaderError>() {
                return reader.source_changed_during_read();
            }
            let Some(next) = source.source() else {
                return false;
            };
            source = next;
        }
    }

    /// Parameter name carried by a stable request refusal.
    #[must_use]
    pub fn parameter(&self) -> Option<&str> {
        match self {
            Self::NoSuchColumn(column) | Self::MixedUnits(column) | Self::BadFilter(column) => {
                Some(column)
            }
            _ => None,
        }
    }

    /// Stable error code used by native transports.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoSuchSegment => "no_such_segment",
            Self::NoSuchSection => "no_such_section",
            Self::NoSuchColumn(_) => "no_such_column",
            Self::MixedUnits(_) => "mixed_units",
            Self::BadFilter(_) => "bad_filter",
            Self::BadCursor => "bad_cursor",
            Self::BadLocator(_) => "bad_locator",
            Self::Cancelled => "cancelled",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSegment => write!(f, "no such segment"),
            Self::NoSuchSection => write!(f, "no such logical section"),
            Self::NoSuchColumn(column) => write!(f, "no such column {column:?}"),
            Self::MixedUnits(fields) => write!(f, "fields carry different units: {fields}"),
            Self::BadFilter(column) => write!(f, "invalid typed filter for {column:?}"),
            Self::BadCursor => write!(f, "invalid page cursor"),
            Self::BadLocator(message) => message.fmt(f),
            Self::Cancelled => write!(f, "request cancelled"),
            Self::Unreadable(error) => error.fmt(f),
        }
    }
}

impl Error for QueryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unreadable(error) => Some(error.as_ref()),
            Self::NoSuchSegment
            | Self::NoSuchSection
            | Self::NoSuchColumn(_)
            | Self::MixedUnits(_)
            | Self::BadFilter(_)
            | Self::BadCursor
            | Self::BadLocator(_)
            | Self::Cancelled => None,
        }
    }
}

impl From<ReaderError> for QueryError {
    fn from(error: ReaderError) -> Self {
        Self::Unreadable(Box::new(error))
    }
}

impl From<serde_json::Error> for QueryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Unreadable(Box::new(error))
    }
}
