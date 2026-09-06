//! Reusable extraction of a bounded time slice from Kronika storage.
//!
//! The binary in this package is only a filesystem adapter. [`slice_to_zms`]
//! reads through the production reader and writes one finished standalone ZMS
//! to a caller-owned sink, so another in-process caller can use the same path.

mod slice;

// The package's inspection adapter uses these shared dependencies.
use chrono as _;
use kronika_index as _;
#[cfg(test)]
use kronika_report as _;
use kronika_store as _;
use serde_json as _;
// The package binary owns its output-parent scratch file.
use tempfile as _;

pub use slice::{RangeError, SliceError, SliceRange, SliceSummary, UtcSecond, slice_to_zms};
