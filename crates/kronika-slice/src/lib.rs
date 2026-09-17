//! Reusable extraction of a bounded time slice from Kronika storage.
//!
//! [`slice_to_zms`] reads through the production reader and writes one finished
//! standalone ZMS using a caller-owned scratch file and output sink. Applications own
//! input configuration, scratch creation and output publication.

mod slice;

pub use slice::{RangeError, SliceError, SliceRange, SliceSummary, UtcSecond, slice_to_zms};
