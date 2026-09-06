//! One appended frame: its header and the part body behind it.

use super::{
    Catalog, CatalogLayoutError, DecodeError, Entry, Error, FRAME_HEADER_LEN, FRAME_MAGIC, MAGIC,
    TAIL_INDEX_LEN, TailIndex, crc32c, fmt, validate_catalog_layout,
};

/// Header of one journal frame.
///
/// The header stores the length of the part body that follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Length of the part body following the header, bytes.
    pub part_len: u64,
}

impl FrameHeader {
    /// Encode this header as its 16-byte on-disk form.
    #[must_use]
    pub fn encode(self) -> [u8; FRAME_HEADER_LEN] {
        let mut out = [0_u8; FRAME_HEADER_LEN];
        out[..4].copy_from_slice(&FRAME_MAGIC);
        out[4..12].copy_from_slice(&self.part_len.to_le_bytes());
        let crc = crc32c(&out[..12]);
        out[12..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// Decode a frame header; validates magic and header CRC.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError`] when the magic bytes or header CRC are invalid.
    pub fn decode(bytes: [u8; FRAME_HEADER_LEN]) -> Result<Self, FrameError> {
        let (meta, stored_crc) = split_header(&bytes);
        if meta[..4] != FRAME_MAGIC {
            let mut actual = [0_u8; 4];
            actual.copy_from_slice(&meta[..4]);
            return Err(FrameError::BadMagic { actual });
        }
        let computed = crc32c(meta);
        if stored_crc != computed {
            return Err(FrameError::BadCrc {
                stored: stored_crc,
                computed,
            });
        }
        let mut len = [0_u8; 8];
        len.copy_from_slice(&meta[4..12]);
        Ok(Self {
            part_len: u64::from_le_bytes(len),
        })
    }
}

/// Split header bytes into the CRC-covered prefix and the stored CRC.
fn split_header(bytes: &[u8; FRAME_HEADER_LEN]) -> (&[u8], u32) {
    let mut crc = [0_u8; 4];
    crc.copy_from_slice(&bytes[12..]);
    (&bytes[..12], u32::from_le_bytes(crc))
}

/// Why frame header bytes failed to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The first four bytes are not [`FRAME_MAGIC`].
    BadMagic {
        /// The bytes actually found.
        actual: [u8; 4],
    },
    /// Stored header CRC32C does not match the computed one.
    BadCrc {
        /// CRC stored in the header.
        stored: u32,
        /// CRC computed over magic + length.
        computed: u32,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { actual } => {
                write!(f, "frame magic is {actual:02x?}, expected \"ZMSP\"")
            }
            Self::BadCrc { stored, computed } => {
                write!(
                    f,
                    "frame header crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
                )
            }
        }
    }
}

impl Error for FrameError {}

/// Why a part body is not a valid ZMS part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartError {
    /// The body is shorter than magic + empty catalog + tail index.
    TooShort {
        /// The byte length actually given.
        actual: usize,
    },
    /// The body does not start with the segment magic.
    BadMagic {
        /// The bytes actually found.
        actual: [u8; 4],
    },
    /// The tail index failed to decode.
    Tail(DecodeError),
    /// `catalog_len` does not fit between the magic and the tail index.
    BadCatalogLen {
        /// `catalog_len` stored in the tail index.
        catalog_len: u32,
    },
    /// The catalog failed to decode.
    Catalog(DecodeError),
    /// The catalog does not describe the canonical physical section layout.
    Layout(CatalogLayoutError),
    /// A catalog entry points outside the section area of the body.
    SectionOutOfBounds {
        /// `type_id` of the entry that failed validation.
        type_id: u32,
    },
    /// A section body does not match its catalog CRC32C.
    SectionCrc {
        /// `type_id` of the entry that failed validation.
        type_id: u32,
        /// CRC stored in the catalog entry.
        stored: u32,
        /// CRC computed over the section body.
        computed: u32,
    },
}

impl fmt::Display for PartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { actual } => {
                write!(f, "part body of {actual} bytes is too short for a ZMS part")
            }
            Self::BadMagic { actual } => {
                write!(f, "part magic is {actual:02x?}, expected \"ZMS1\"")
            }
            Self::Tail(err) => write!(f, "part tail index: {err}"),
            Self::BadCatalogLen { catalog_len } => {
                write!(f, "part catalog_len {catalog_len} does not fit the body")
            }
            Self::Catalog(err) => write!(f, "part catalog: {err}"),
            Self::Layout(err) => write!(f, "part section layout: {err}"),
            Self::SectionOutOfBounds { type_id } => {
                write!(f, "section {type_id} points outside the part body")
            }
            Self::SectionCrc {
                type_id,
                stored,
                computed,
            } => {
                write!(
                    f,
                    "section {type_id} crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
                )
            }
        }
    }
}

impl Error for PartError {}

/// Validate a self-contained ZMS part, including section CRCs.
///
/// # Errors
///
/// Returns [`PartError`] when framing, catalog, section bounds, or section CRC
/// checks fail.
pub fn validate_part(bytes: &[u8]) -> Result<Catalog, PartError> {
    let catalog = decode_and_bound(bytes)?;
    for entry in &catalog.entries {
        // `decode_and_bound` confirmed every section is in range, so the casts
        // and the slice are safe.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "offset and len fit in usize: both are bounded by the part length"
        )]
        let body = &bytes[entry.offset as usize..(entry.offset + entry.len) as usize];
        let computed = crc32c(body);
        if computed != entry.crc32c {
            return Err(PartError::SectionCrc {
                type_id: entry.type_id,
                stored: entry.crc32c,
                computed,
            });
        }
    }
    Ok(catalog)
}

/// Validate part framing and catalog without hashing section bodies.
///
/// Use only when section CRCs are checked elsewhere.
///
/// # Errors
///
/// Returns [`PartError`] when framing, catalog, or section bounds checks fail.
pub fn validate_part_catalog(bytes: &[u8]) -> Result<Catalog, PartError> {
    decode_and_bound(bytes)
}

/// Decode a part catalog and confirm section bounds.
fn decode_and_bound(bytes: &[u8]) -> Result<Catalog, PartError> {
    // Smallest possible part: magic + empty catalog (meta only) + tail.
    let min_len = MAGIC.len() + crate::META_LEN + TAIL_INDEX_LEN;
    if bytes.len() < min_len {
        return Err(PartError::TooShort {
            actual: bytes.len(),
        });
    }
    if bytes[..4] != MAGIC {
        let mut actual = [0_u8; 4];
        actual.copy_from_slice(&bytes[..4]);
        return Err(PartError::BadMagic { actual });
    }

    let mut tail_bytes = [0_u8; TAIL_INDEX_LEN];
    tail_bytes.copy_from_slice(&bytes[bytes.len() - TAIL_INDEX_LEN..]);
    let tail = TailIndex::decode(tail_bytes).map_err(PartError::Tail)?;

    let catalog_len = tail.catalog_len as usize;
    let body_end = bytes.len() - TAIL_INDEX_LEN;
    let Some(catalog_start) = body_end.checked_sub(catalog_len) else {
        return Err(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        });
    };
    if catalog_start < MAGIC.len() {
        return Err(PartError::BadCatalogLen {
            catalog_len: tail.catalog_len,
        });
    }

    let catalog = Catalog::decode(&bytes[catalog_start..body_end]).map_err(PartError::Catalog)?;

    validate_catalog_layout(&catalog, catalog_start as u64).map_err(PartError::Layout)?;

    Ok(catalog)
}

/// One opaque section body to place in a part.
#[derive(Debug, Clone, Copy)]
pub struct SectionInput<'a> {
    /// Section type from the type registry (`kronika-registry`).
    pub type_id: u32,
    /// Number of rows or records the body holds; recorded in the catalog.
    pub rows: u32,
    /// The section body bytes, placed verbatim.
    pub body: &'a [u8],
}

/// Segment-level catalog metadata for a part, the fields not derivable from the
/// section bodies.
#[derive(Debug, Clone, Copy)]
pub struct PartMeta {
    /// Minimal timestamp across the part's rows, unix microseconds.
    pub min_ts: i64,
    /// Maximal timestamp across the part's rows, unix microseconds.
    pub max_ts: i64,
}

/// Assemble section bodies into a self-contained ZMS part.
///
/// Offsets and CRCs are computed here.
///
/// # Panics
///
/// If the encoded catalog block does not fit in `u32`.
#[must_use]
pub fn build_part(sections: &[SectionInput<'_>], meta: PartMeta) -> Vec<u8> {
    // The exact part length is known up front.
    let bodies: usize = sections.iter().map(|section| section.body.len()).sum();
    let capacity =
        MAGIC.len() + bodies + sections.len() * crate::ENTRY_LEN + crate::META_LEN + TAIL_INDEX_LEN;
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(&MAGIC);

    let entries = sections
        .iter()
        .map(|section| {
            // Catalog offsets are absolute from the part start.
            let offset = out.len() as u64;
            out.extend_from_slice(section.body);
            Entry {
                type_id: section.type_id,
                flags: 0,
                offset,
                len: section.body.len() as u64,
                rows: section.rows,
                crc32c: crc32c(section.body),
            }
        })
        .collect();

    let catalog = Catalog {
        entries,
        min_ts: meta.min_ts,
        max_ts: meta.max_ts,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    };
    catalog
        .write_encoded(&mut out)
        .expect("catalog length fits the precomputed part capacity");
    out
}
