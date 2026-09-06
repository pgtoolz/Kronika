//! End catalog and tail index.
//!
//! A reader opens a segment from the end. The last 8 bytes are the tail
//! index; it gives the byte length of the catalog block immediately before
//! it. The catalog block contains fixed-size entries followed by fixed-size
//! segment metadata.
//!
//! All integers are little-endian. Catalog entry offsets are absolute file
//! offsets from the start of the segment.
//!
//! ```text
//! catalog entry: 32 B       metadata: 32 B            tail index: 8 B
//!   type_id        u32        min_ts          i64       catalog_len u32
//!   flags          u32        max_ts          i64       magic       "ZMS1"
//!   offset         u64        entry_count     u32
//!   len            u64        format_version  u32
//!   rows           u32        crc32c          u32
//!   crc32c         u32        window_count    u32
//! ```

use std::error::Error;
use std::fmt;
use std::io::{self, Write};

use crate::{MAGIC, MAX_JOURNAL_LEN, crc::Crc32c};

mod decode;
mod layout;

pub use decode::DecodeError;
use decode::{decode_entry, i64_at, u32_at};
pub use layout::{CatalogLayoutError, validate_catalog_layout};

/// Size of one catalog entry on disk, bytes.
pub const ENTRY_LEN: usize = 32;
/// Size of the catalog meta block on disk, bytes.
pub const META_LEN: usize = 32;
/// Size of the tail index on disk, bytes. Always the last bytes of a file.
pub const TAIL_INDEX_LEN: usize = 8;

/// Offset of the `crc32c` field inside the meta block.
const META_CRC_OFFSET: usize = 24;

/// One row in the end catalog.
///
/// Each row points to one section body and records the checksum of that body.
/// Physical readers additionally require the canonical ordering checked by
/// [`validate_catalog_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Section type from the type registry (`kronika-registry`).
    pub type_id: u32,
    /// Reserved, written as zero.
    pub flags: u32,
    /// Absolute offset of the section body from the start of the file.
    pub offset: u64,
    /// Length of the section body, bytes.
    pub len: u64,
    /// Number of rows or records in the section.
    pub rows: u32,
    /// CRC32C of the section body.
    pub crc32c: u32,
}

/// Decoded end catalog.
///
/// The catalog contains all section entries and the segment-level metadata
/// stored after those entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// Section table in canonical physical order.
    pub entries: Vec<Entry>,
    /// Minimal timestamp of the segment, unix microseconds.
    pub min_ts: i64,
    /// Maximal timestamp of the segment, unix microseconds.
    pub max_ts: i64,
    /// Container format version, [`crate::FORMAT_VERSION`] for new files.
    pub format_version: u32,
    /// Collection windows coalesced into this container; 0 = unknown.
    pub window_count: u32,
}

/// Validated borrowed view of an encoded end catalog.
///
/// Unlike [`Catalog`], this type does not allocate or retain decoded entries.
/// It is intended for bounded discovery paths that need to validate and
/// summarize a catalog before deciding whether to open the complete segment.
#[derive(Debug, Clone, Copy)]
pub struct CatalogView<'a> {
    entries: &'a [u8],
    /// Minimal timestamp of the segment, unix microseconds.
    pub min_ts: i64,
    /// Maximal timestamp of the segment, unix microseconds.
    pub max_ts: i64,
    /// Number of catalog entries.
    pub entry_count: u32,
    /// Container format version.
    pub format_version: u32,
    /// Collection windows coalesced into this container; 0 = unknown.
    pub window_count: u32,
}

impl CatalogView<'_> {
    /// Iterates over decoded entries without allocating a `Vec<Entry>`.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = Entry> + Clone + '_ {
        self.entries.chunks_exact(ENTRY_LEN).map(decode_entry)
    }
}

/// Pointer to the end catalog.
///
/// This is always the last 8 bytes of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailIndex {
    /// Length of the catalog (entries + meta) preceding the tail index.
    pub catalog_len: u32,
}

const DICT_STRINGS_TYPE_ID: u32 = 3_001_001;
const DICT_BLOBS_TYPE_ID: u32 = 3_002_001;
/// Hard version-1 length limit for one finished section body.
pub const MAX_PHYSICAL_SECTION_BYTES: u64 = MAX_JOURNAL_LEN as u64;

impl TailIndex {
    /// Encode this tail index as its 8-byte on-disk form.
    #[must_use]
    pub fn encode(self) -> [u8; TAIL_INDEX_LEN] {
        let mut out = [0_u8; TAIL_INDEX_LEN];
        out[..4].copy_from_slice(&self.catalog_len.to_le_bytes());
        out[4..].copy_from_slice(&MAGIC);
        out
    }

    /// Decode the final 8 bytes of a segment.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BadTailMagic`] when the trailing magic bytes are
    /// not `ZMS1`.
    pub fn decode(bytes: [u8; TAIL_INDEX_LEN]) -> Result<Self, DecodeError> {
        let [l0, l1, l2, l3, m0, m1, m2, m3] = bytes;
        let magic = [m0, m1, m2, m3];
        if magic != MAGIC {
            return Err(DecodeError::BadTailMagic { actual: magic });
        }
        let catalog_len = u32::from_le_bytes([l0, l1, l2, l3]);
        Ok(Self { catalog_len })
    }
}

impl Catalog {
    /// Length of the catalog block, excluding the tail index.
    ///
    /// This is the value stored as `catalog_len` in [`TailIndex`].
    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        self.entries.len() * ENTRY_LEN + META_LEN
    }

    /// Encode catalog entries, metadata, and the tail index.
    ///
    /// The returned buffer starts immediately after the last section body and
    /// ends with the 8-byte tail index.
    ///
    /// # Panics
    ///
    /// Panics if the encoded catalog block does not fit in `u32`. That is a
    /// writer bug: a valid segment cannot address a larger catalog block.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len() + TAIL_INDEX_LEN);
        self.write_encoded(&mut out)
            .expect("catalog length must fit u32 and writing to Vec cannot fail");
        out
    }

    /// Write catalog entries, metadata, and the tail index without allocating
    /// a second catalog-sized buffer.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the catalog block does not
    /// fit in `u32`, or forwards an error from `writer`.
    pub fn write_encoded(&self, mut writer: impl Write) -> io::Result<()> {
        let catalog_len = u32::try_from(self.encoded_len()).map_err(|_overflow| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "catalog length does not fit the ZMS tail index",
            )
        })?;
        let mut checksum = Crc32c::new();
        for e in &self.entries {
            let mut bytes = [0_u8; ENTRY_LEN];
            bytes[0..4].copy_from_slice(&e.type_id.to_le_bytes());
            bytes[4..8].copy_from_slice(&e.flags.to_le_bytes());
            bytes[8..16].copy_from_slice(&e.offset.to_le_bytes());
            bytes[16..24].copy_from_slice(&e.len.to_le_bytes());
            bytes[24..28].copy_from_slice(&e.rows.to_le_bytes());
            bytes[28..32].copy_from_slice(&e.crc32c.to_le_bytes());
            checksum.update(&bytes);
            writer.write_all(&bytes)?;
        }

        let mut meta = [0_u8; META_LEN];
        meta[0..8].copy_from_slice(&self.min_ts.to_le_bytes());
        meta[8..16].copy_from_slice(&self.max_ts.to_le_bytes());
        let entry_count = u32::try_from(self.entries.len()).map_err(|_overflow| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "catalog entry count does not fit the ZMS metadata",
            )
        })?;
        meta[16..20].copy_from_slice(&entry_count.to_le_bytes());
        meta[20..24].copy_from_slice(&self.format_version.to_le_bytes());
        meta[28..32].copy_from_slice(&self.window_count.to_le_bytes());
        // The CRC field is already zeroed.
        checksum.update(&meta);
        meta[META_CRC_OFFSET..META_CRC_OFFSET + 4]
            .copy_from_slice(&checksum.finalize().to_le_bytes());
        writer.write_all(&meta)?;
        writer.write_all(&TailIndex { catalog_len }.encode())
    }

    /// Decode a catalog block.
    ///
    /// `bytes` must contain catalog entries followed by the 32-byte metadata
    /// block. Do not include the tail index.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] when the block length is impossible, the
    /// stored entry count does not match the block length, or the catalog CRC
    /// does not match.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let view = Self::view(bytes)?;
        Ok(Self {
            entries: view.entries().collect(),
            min_ts: view.min_ts,
            max_ts: view.max_ts,
            format_version: view.format_version,
            window_count: view.window_count,
        })
    }

    /// Validate an encoded catalog and borrow its fixed-size entries.
    ///
    /// `bytes` must contain catalog entries followed by the 32-byte metadata
    /// block. The returned view decodes entries on iteration and allocates
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeError`] under the same conditions as [`Self::decode`].
    pub fn view(bytes: &[u8]) -> Result<CatalogView<'_>, DecodeError> {
        if bytes.len() < META_LEN || !(bytes.len() - META_LEN).is_multiple_of(ENTRY_LEN) {
            return Err(DecodeError::BadCatalogLen {
                actual: bytes.len(),
            });
        }
        // The only possible error is overflow of an absurd length; the
        // original `TryFromIntError` carries nothing worth keeping.
        let derived = u32::try_from((bytes.len() - META_LEN) / ENTRY_LEN).map_err(|_overflow| {
            DecodeError::BadCatalogLen {
                actual: bytes.len(),
            }
        })?;

        let meta = &bytes[bytes.len() - META_LEN..];
        let stored_count = u32_at(meta, 16);
        if stored_count != derived {
            return Err(DecodeError::EntryCountMismatch {
                stored: stored_count,
                derived,
            });
        }

        // The CRC field participates in the checksum as zeroes. Computed
        // incrementally: decode runs once per part during journal recovery,
        // and cloning the whole catalog block here would double the peak
        // memory of that path.
        let stored_crc = u32_at(meta, META_CRC_OFFSET);
        let crc_at = bytes.len() - META_LEN + META_CRC_OFFSET;
        let computed = crate::crc::crc32c_with_zeroed_field(bytes, crc_at);
        if stored_crc != computed {
            return Err(DecodeError::BadCrc {
                stored: stored_crc,
                computed,
            });
        }

        Ok(CatalogView {
            entries: &bytes[..bytes.len() - META_LEN],
            min_ts: i64_at(meta, 0),
            max_ts: i64_at(meta, 8),
            entry_count: stored_count,
            format_version: u32_at(meta, 20),
            window_count: u32_at(meta, 28),
        })
    }
}

#[cfg(test)]
mod tests;
