//! The `Section` trait: the typed codec contract `#[derive(Section)]` writes.
//!
//! Typed access when the concrete section type is known.

use crate::codec::{CodecError, VerifiedSection};
use crate::contract::TypeContract;

/// A section type: its registry contract plus the Parquet codec for its rows.
///
/// Closed to downstream impls; only this crate's derive can implement it.
pub trait Section: crate::private::Private + Sized {
    /// The registry contract for this type.
    const CONTRACT: TypeContract;

    /// Encode `rows` into a Parquet section body.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when rows exceed caps or Parquet encoding fails.
    fn encode(rows: &[Self]) -> Result<Vec<u8>, CodecError>;

    /// Decode a verified section body back into typed rows.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError`] when schema, caps, or Parquet decoding fail.
    fn decode(section: VerifiedSection) -> Result<Vec<Self>, CodecError>;

    /// Timestamp range for catalog metadata.
    fn ts_range(rows: &[Self]) -> Option<(i64, i64)>;

    /// Total child values stored by all `ListI32` columns in `rows`.
    ///
    /// The generated implementation saturates at [`usize::MAX`] so callers
    /// performing bounded admission cannot underestimate work on overflow.
    fn list_i32_child_value_count(rows: &[Self]) -> usize;
}

/// Shared roundtrip assertion for generated codecs.
#[cfg(test)]
pub(crate) fn assert_roundtrips<T>(rows: &[T])
where
    T: Section + PartialEq + std::fmt::Debug,
{
    let bytes = T::encode(rows).expect("encode");
    let decoded = T::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
    assert_eq!(decoded.as_slice(), rows);
}
