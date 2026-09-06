//! Storage-independent ZMS framing and catalog validation.

use std::io;

use kronika_format::{
    Catalog, CatalogLayoutError, Crc32c, DecodeError, Entry, FORMAT_VERSION, MAGIC, ReadAt,
    TAIL_INDEX_LEN, TailIndex, validate_catalog_layout,
};

use crate::{CatalogSummary, ResourceError};

pub(crate) const MAX_CATALOG_BYTES: u64 = 64 * 1024 * 1024;
const CRC_CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug)]
pub(crate) enum ZmsError {
    Io(io::Error),
    MetadataLimit { limit: usize },
    TooSmall,
    BadMagic,
    TailIndex(DecodeError),
    UnsupportedFormat { version: u32 },
    BadCatalogLength,
    Catalog(DecodeError),
    SectionLayout(CatalogLayoutError),
    SectionChecksum { type_id: u32 },
}

impl From<io::Error> for ZmsError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FinishedValidation {
    #[cfg(feature = "posix")]
    Catalog,
    Complete,
}

struct EncodedCatalog {
    bytes: Vec<u8>,
    body_end: u64,
}

const fn metadata_limit(limit: usize) -> ZmsError {
    ZmsError::MetadataLimit { limit }
}

fn read_encoded_catalog<R: ReadAt>(
    reader: &R,
    metadata_budget: Option<(usize, usize)>,
) -> Result<EncodedCatalog, ZmsError> {
    let len = reader.byte_len()?;
    let tail_at = len
        .checked_sub(TAIL_INDEX_LEN as u64)
        .ok_or(ZmsError::TooSmall)?;

    let mut tail_bytes = [0_u8; TAIL_INDEX_LEN];
    reader.read_exact_at(&mut tail_bytes, tail_at)?;
    let tail = TailIndex::decode(tail_bytes).map_err(ZmsError::TailIndex)?;

    let catalog_len = u64::from(tail.catalog_len);
    if catalog_len > MAX_CATALOG_BYTES {
        return Err(ZmsError::BadCatalogLength);
    }
    let catalog_at = tail_at
        .checked_sub(catalog_len)
        .ok_or(ZmsError::BadCatalogLength)?;
    if catalog_at < MAGIC.len() as u64 {
        return Err(ZmsError::BadCatalogLength);
    }
    let catalog_bytes =
        usize::try_from(catalog_len).map_err(|_overflow| ZmsError::BadCatalogLength)?;
    if let Some((retained, limit)) = metadata_budget
        && retained
            .checked_add(catalog_bytes)
            .is_none_or(|peak| peak > limit)
    {
        return Err(metadata_limit(limit));
    }

    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(catalog_bytes)
        .map_err(|_error| ZmsError::Io(io::Error::from(io::ErrorKind::OutOfMemory)))?;
    bytes.resize(catalog_bytes, 0);
    reader.read_exact_at(&mut bytes, catalog_at)?;
    Ok(EncodedCatalog {
        bytes,
        body_end: catalog_at,
    })
}

pub(crate) fn read_zms_summary<R: ReadAt>(
    reader: &R,
    retained_metadata: usize,
    metadata_limit_bytes: usize,
    validation: FinishedValidation,
) -> Result<CatalogSummary, ZmsError> {
    let encoded = read_encoded_catalog(reader, Some((retained_metadata, metadata_limit_bytes)))?;
    verify_magic(reader)?;
    let view = Catalog::view(&encoded.bytes).map_err(ZmsError::Catalog)?;
    let entry_allocation = view
        .entries()
        .len()
        .checked_mul(size_of::<Entry>())
        .ok_or(ZmsError::BadCatalogLength)?;
    let transient = retained_metadata
        .checked_add(encoded.bytes.len())
        .and_then(|bytes| bytes.checked_add(entry_allocation))
        .ok_or_else(|| metadata_limit(metadata_limit_bytes))?;
    if transient > metadata_limit_bytes {
        return Err(metadata_limit(metadata_limit_bytes));
    }
    let catalog = Catalog {
        entries: view.entries().collect(),
        min_ts: view.min_ts,
        max_ts: view.max_ts,
        format_version: view.format_version,
        window_count: view.window_count,
    };
    if catalog.format_version != FORMAT_VERSION {
        return Err(ZmsError::UnsupportedFormat {
            version: catalog.format_version,
        });
    }
    validate_catalog_layout(&catalog, encoded.body_end).map_err(ZmsError::SectionLayout)?;
    if matches!(validation, FinishedValidation::Complete) {
        validate_section_checksums(reader, &catalog)?;
    }
    let catalog_len =
        u32::try_from(encoded.bytes.len()).map_err(|_overflow| ZmsError::BadCatalogLength)?;
    Ok(CatalogSummary::from_catalog(&catalog, catalog_len))
}

/// Validate one complete finished ZMS through the storage-independent reader.
///
/// This checks the size admission bound, framing, canonical catalog geometry,
/// and every section checksum without retaining section bodies.
///
/// # Errors
///
/// Returns [`ResourceError`] when the object exceeds `max_segment_bytes` or
/// any required byte, catalog field, layout relation, or checksum is invalid.
pub fn validate_finished_zms<R: ReadAt>(
    reader: &R,
    max_segment_bytes: u64,
) -> Result<CatalogSummary, ResourceError> {
    let len = reader
        .byte_len()
        .map_err(|problem| ResourceError::from_io(&problem))?;
    if len > max_segment_bytes {
        return Err(ResourceError::TooLarge {
            len,
            max: max_segment_bytes,
        });
    }
    read_zms_summary(
        reader,
        0,
        kronika_layout::LayoutLimits::default().max_metadata_bytes,
        FinishedValidation::Complete,
    )
    .map_err(ResourceError::from_zms)
}

fn validate_section_checksums<R: ReadAt>(reader: &R, catalog: &Catalog) -> Result<(), ZmsError> {
    let mut buffer = [0_u8; CRC_CHUNK_BYTES];
    for entry in &catalog.entries {
        let mut checksum = Crc32c::new();
        let mut offset = entry.offset;
        let mut remaining = entry.len;
        while remaining != 0 {
            let chunk_len =
                usize::try_from(remaining.min(CRC_CHUNK_BYTES as u64)).map_err(|_overflow| {
                    ZmsError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ZMS checksum chunk length overflow",
                    ))
                })?;
            reader.read_exact_at(&mut buffer[..chunk_len], offset)?;
            checksum.update(&buffer[..chunk_len]);
            offset = offset.checked_add(chunk_len as u64).ok_or_else(|| {
                ZmsError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ZMS checksum offset overflow",
                ))
            })?;
            remaining -= chunk_len as u64;
        }
        if checksum.finalize() != entry.crc32c {
            return Err(ZmsError::SectionChecksum {
                type_id: entry.type_id,
            });
        }
    }
    Ok(())
}

fn verify_magic<R: ReadAt>(reader: &R) -> Result<(), ZmsError> {
    let mut magic = [0_u8; MAGIC.len()];
    reader.read_exact_at(&mut magic, 0)?;
    if magic == MAGIC {
        Ok(())
    } else {
        Err(ZmsError::BadMagic)
    }
}

pub(crate) fn read_catalog<R: ReadAt>(reader: &R) -> Result<Catalog, ZmsError> {
    let encoded = read_encoded_catalog(reader, None)?;
    let catalog = Catalog::decode(&encoded.bytes).map_err(ZmsError::Catalog)?;
    verify_magic(reader)?;
    if catalog.format_version != FORMAT_VERSION {
        return Err(ZmsError::UnsupportedFormat {
            version: catalog.format_version,
        });
    }
    validate_catalog_layout(&catalog, encoded.body_end).map_err(ZmsError::SectionLayout)?;
    Ok(catalog)
}
