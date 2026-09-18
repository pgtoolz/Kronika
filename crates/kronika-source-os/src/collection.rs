//! Errors from bounded procfs acquisition and parsing.

use crate::proc::stat::ParseError;
use std::io;

/// A source could not be read or its counters could not be parsed.
#[derive(Debug)]
pub enum CollectionError {
    /// Root-confined, bounded source read failed.
    Read(io::Error),
    /// Source contents were invalid.
    Parse(ParseError),
}

impl CollectionError {
    /// Whether an optional source is absent rather than unreadable or malformed.
    #[must_use]
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Read(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

impl std::fmt::Display for CollectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CollectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

impl From<io::Error> for CollectionError {
    fn from(error: io::Error) -> Self {
        Self::Read(error)
    }
}

impl From<ParseError> for CollectionError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}
