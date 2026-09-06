//! Interned string ids.
//!
//! Text values in a segment are referenced by `str_id = xxh3_64(bytes)` and
//! stored once in the segment dictionaries.

use std::num::NonZeroU64;

use xxhash_rust::xxh3::xxh3_64;

/// Interned string id: `xxh3_64` of the original value bytes.
///
/// On disk, `0` means "no value". A real `StrId` is therefore always
/// non-zero. If a value hashes to zero, the writer treats that input as a
/// collision and does not add it to the dictionaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrId(NonZeroU64);

impl StrId {
    /// Hash `bytes` with `xxh3_64`.
    ///
    /// The hash is computed over the raw value bytes. Returns `None` when
    /// the hash is `0`, the on-disk sentinel for "no value".
    #[must_use]
    pub fn of(bytes: &[u8]) -> Option<Self> {
        NonZeroU64::new(xxh3_64(bytes)).map(Self)
    }

    /// Return the raw `u64` stored on disk.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Convert a raw on-disk id.
    ///
    /// Returns `None` for `0`, the on-disk sentinel for "no value".
    #[must_use]
    pub const fn from_raw(raw: u64) -> Option<Self> {
        match NonZeroU64::new(raw) {
            Some(id) => Some(Self(id)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StrId;

    /// The canonical `XXH3_64bits` check value for empty input from the
    /// xxHash reference implementation. Catches accidentally swapping the
    /// algorithm (e.g. plain xxh64) the same way the CRC32C known vector
    /// does for checksums.
    #[test]
    fn known_vector_empty_input() {
        let id = StrId::of(b"").expect("empty input must hash to a non-zero id");
        assert_eq!(id.get(), 0x2D06_8005_38D3_94C2);
    }

    #[test]
    fn raw_roundtrip_and_zero_sentinel() {
        let id = StrId::of(b"pg_stat_activity").expect("non-zero id");
        assert_eq!(StrId::from_raw(id.get()), Some(id));
        assert_eq!(StrId::from_raw(0), None);
    }
}
