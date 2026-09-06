//! Decoding a catalog block back into entries.

use super::{ENTRY_LEN, Entry, Error, META_LEN, fmt};

/// Why catalog or tail index bytes failed to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The last four bytes of the tail index are not [`MAGIC`].
    BadTailMagic {
        /// The bytes actually found.
        actual: [u8; 4],
    },
    /// Catalog byte length is not `entries × 32 + 40`.
    BadCatalogLen {
        /// The byte length actually given.
        actual: usize,
    },
    /// `entry_count` in meta does not match the byte length.
    EntryCountMismatch {
        /// Entry count stored in meta.
        stored: u32,
        /// Entry count implied by the byte length.
        derived: u32,
    },
    /// Stored catalog CRC32C does not match the computed one.
    BadCrc {
        /// CRC stored in meta.
        stored: u32,
        /// CRC computed over the bytes.
        computed: u32,
    },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadTailMagic { actual } => {
                write!(f, "tail index magic is {actual:02x?}, expected \"ZMS1\"")
            }
            Self::BadCatalogLen { actual } => {
                write!(
                    f,
                    "catalog length {actual} is not entries x {ENTRY_LEN} + {META_LEN}"
                )
            }
            Self::EntryCountMismatch { stored, derived } => {
                write!(
                    f,
                    "entry_count in meta is {stored}, but byte length implies {derived}"
                )
            }
            Self::BadCrc { stored, computed } => {
                write!(
                    f,
                    "catalog crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
                )
            }
        }
    }
}

impl Error for DecodeError {}

pub(super) fn decode_entry(bytes: &[u8]) -> Entry {
    Entry {
        type_id: u32_at(bytes, 0),
        flags: u32_at(bytes, 4),
        offset: u64_at(bytes, 8),
        len: u64_at(bytes, 16),
        rows: u32_at(bytes, 24),
        crc32c: u32_at(bytes, 28),
    }
}

pub(super) fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("caller checked bounds"))
}

pub(super) fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("caller checked bounds"))
}

pub(super) fn i64_at(bytes: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(bytes[at..at + 8].try_into().expect("caller checked bounds"))
}
