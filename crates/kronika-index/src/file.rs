//! The single current `.idx` container format.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use kronika_format::Crc32c;

use crate::decode::u32_at;
use crate::series::{SeriesBlock, SeriesKey, SeriesKind};

/// Magic for the current derived-index format.
pub const MAGIC: [u8; 8] = *b"KRNIDX1\0";
/// Bytes before the table: magic, entry count, checksum.
pub const HEADER_LEN: usize = 16;
/// Bytes per TOC entry: series kind, physical input layout, offset, length.
pub const ENTRY_LEN: usize = 16;

const CHECKSUM_AT: usize = 12;
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
// Ten presentation blocks and 25 locator blocks are currently allowlisted.
const MAX_BLOCKS: usize = 36;
const CHECKSUM_CHUNK: usize = 16 * 1024;

/// Why an index container or selected block was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexError {
    /// A declared byte range is absent.
    Truncated,
    /// The file is not the one current format.
    BadMagic,
    /// The persisted checksum does not match the bytes.
    BadChecksum,
    /// TOC order, a block, or a bounded count is invalid.
    BadLayout,
    /// The file exceeds the fixed index read bound.
    TooLarge,
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the index file is truncated"),
            Self::BadMagic => write!(f, "the index file does not start with its magic"),
            Self::BadChecksum => write!(f, "the index file does not match its checksum"),
            Self::BadLayout => write!(f, "the index table or selected block is invalid"),
            Self::TooLarge => write!(f, "the index file exceeds its read bound"),
        }
    }
}

impl std::error::Error for IndexError {}

/// All allowlisted blocks built for one segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// Canonically ordered presentation blocks.
    pub blocks: Vec<SeriesBlock>,
}

/// Selected blocks from a validated index.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetedIndex {
    /// Canonical checksum suitable for a finished response `ETag`; active
    /// computed resources have no checksum.
    pub checksum: Option<u32>,
    /// Selected presentation blocks.
    pub blocks: Vec<SeriesBlock>,
}

impl TargetedIndex {
    /// Whether selected blocks cover every required requested key.
    ///
    /// `PostgresHealth` is optional because a valid index omits it when
    /// `PostgreSQL` collection was disabled for the segment.
    #[must_use]
    pub fn contains_targets(&self, keys: &[SeriesKey]) -> bool {
        keys.iter()
            .filter(|key| !matches!(key.kind, SeriesKind::PostgresHealth))
            .all(|key| self.blocks.iter().any(|block| block.key() == *key))
    }
}

#[derive(Debug, Clone, Copy)]
struct TocEntry {
    key: SeriesKey,
    offset: u32,
    len: u32,
}

impl Index {
    /// Encode canonical bytes of the only current index format.
    ///
    /// # Errors
    ///
    /// Returns a layout or size failure.
    pub fn encode(&self) -> Result<Vec<u8>, IndexError> {
        if self.blocks.len() > MAX_BLOCKS {
            return Err(IndexError::TooLarge);
        }
        let count = u32::try_from(self.blocks.len()).map_err(|_overflow| IndexError::TooLarge)?;
        let table_len = self
            .blocks
            .len()
            .checked_mul(ENTRY_LEN)
            .ok_or(IndexError::TooLarge)?;
        let body_at = HEADER_LEN
            .checked_add(table_len)
            .ok_or(IndexError::TooLarge)?;
        let mut bytes = Vec::with_capacity(body_at);
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.resize(body_at, 0);

        let mut previous = None;
        let mut offset = 0_u32;
        for (index, block) in self.blocks.iter().enumerate() {
            let key = block.key();
            if previous.is_some_and(|before| before >= key) {
                return Err(IndexError::BadLayout);
            }
            previous = Some(key);
            let body = block.encode()?;
            let len = u32::try_from(body.len()).map_err(|_overflow| IndexError::TooLarge)?;
            let table_at = HEADER_LEN
                .checked_add(index.checked_mul(ENTRY_LEN).ok_or(IndexError::TooLarge)?)
                .ok_or(IndexError::TooLarge)?;
            put_u32_at(&mut bytes, table_at, key.kind as u32)?;
            put_u32_at(&mut bytes, table_at + 4, key.type_id)?;
            put_u32_at(&mut bytes, table_at + 8, offset)?;
            put_u32_at(&mut bytes, table_at + 12, len)?;
            offset = offset.checked_add(len).ok_or(IndexError::TooLarge)?;
            let total = bytes
                .len()
                .checked_add(body.len())
                .ok_or(IndexError::TooLarge)?;
            if u64::try_from(total).map_err(|_overflow| IndexError::TooLarge)? > MAX_INDEX_BYTES {
                return Err(IndexError::TooLarge);
            }
            bytes.extend_from_slice(&body);
        }

        let value = checksum(&bytes[..CHECKSUM_AT], &bytes[HEADER_LEN..]);
        bytes[CHECKSUM_AT..HEADER_LEN].copy_from_slice(&value.to_le_bytes());
        Ok(bytes)
    }

    /// Decode every block.
    ///
    /// # Errors
    ///
    /// Returns the first container or block validation failure.
    pub fn decode(bytes: &[u8]) -> Result<Self, IndexError> {
        read_all(&mut std::io::Cursor::new(bytes))
    }

    /// Decode only requested blocks while validating the complete container.
    ///
    /// # Errors
    ///
    /// Returns the first container or selected-block validation failure.
    pub fn decode_target(bytes: &[u8], keys: &[SeriesKey]) -> Result<TargetedIndex, IndexError> {
        read_target(&mut std::io::Cursor::new(bytes), keys)
    }
}

pub(crate) fn read_target(
    reader: &mut (impl Read + Seek),
    keys: &[SeriesKey],
) -> Result<TargetedIndex, IndexError> {
    let (expected_checksum, table, body_at, file_len) = metadata(reader)?;
    validate_checksum(reader, expected_checksum, file_len)?;
    let wanted: HashSet<SeriesKey> = keys.iter().copied().collect();
    let mut blocks = Vec::new();
    for entry in table {
        if !wanted.contains(&entry.key) {
            continue;
        }
        blocks.push(read_block(reader, body_at, entry)?);
    }
    Ok(TargetedIndex {
        checksum: Some(expected_checksum),
        blocks,
    })
}

pub(crate) fn read_all(reader: &mut (impl Read + Seek)) -> Result<Index, IndexError> {
    let (expected_checksum, table, body_at, file_len) = metadata(reader)?;
    validate_checksum(reader, expected_checksum, file_len)?;
    let mut blocks = Vec::with_capacity(table.len());
    for entry in table {
        blocks.push(read_block(reader, body_at, entry)?);
    }
    Ok(Index { blocks })
}

fn read_block(
    reader: &mut (impl Read + Seek),
    body_at: u64,
    entry: TocEntry,
) -> Result<SeriesBlock, IndexError> {
    let absolute = body_at
        .checked_add(u64::from(entry.offset))
        .ok_or(IndexError::BadLayout)?;
    reader
        .seek(SeekFrom::Start(absolute))
        .map_err(|_error| IndexError::Truncated)?;
    let len = usize::try_from(entry.len).map_err(|_overflow| IndexError::TooLarge)?;
    let mut block = vec![0_u8; len];
    reader
        .read_exact(&mut block)
        .map_err(|_error| IndexError::Truncated)?;
    SeriesBlock::decode(entry.key, &block)
}

fn metadata(reader: &mut (impl Read + Seek)) -> Result<(u32, Vec<TocEntry>, u64, u64), IndexError> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_error| IndexError::Truncated)?;
    if file_len > MAX_INDEX_BYTES {
        return Err(IndexError::TooLarge);
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_error| IndexError::Truncated)?;
    let mut header = [0_u8; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_error| IndexError::Truncated)?;
    if header[..MAGIC.len()] != MAGIC {
        return Err(IndexError::BadMagic);
    }
    let count = usize::try_from(u32_at(&header, 8)?).map_err(|_overflow| IndexError::TooLarge)?;
    if count > MAX_BLOCKS {
        return Err(IndexError::TooLarge);
    }
    let expected_checksum = u32_at(&header, CHECKSUM_AT)?;
    let table_len = count.checked_mul(ENTRY_LEN).ok_or(IndexError::TooLarge)?;
    let body_at = u64::try_from(HEADER_LEN)
        .ok()
        .and_then(|header| {
            u64::try_from(table_len)
                .ok()
                .and_then(|table| header.checked_add(table))
        })
        .ok_or(IndexError::TooLarge)?;
    if body_at > file_len {
        return Err(IndexError::Truncated);
    }
    let mut raw_table = vec![0_u8; table_len];
    reader
        .read_exact(&mut raw_table)
        .map_err(|_error| IndexError::Truncated)?;
    let mut table = Vec::with_capacity(count);
    let mut expected_offset = 0_u32;
    let mut previous = None;
    for raw in raw_table.chunks_exact(ENTRY_LEN) {
        let key = SeriesKey {
            kind: SeriesKind::from_raw(u32_at(raw, 0)?)?,
            type_id: u32_at(raw, 4)?,
        };
        let entry = TocEntry {
            key,
            offset: u32_at(raw, 8)?,
            len: u32_at(raw, 12)?,
        };
        if entry.offset != expected_offset || previous.is_some_and(|before| before >= key) {
            return Err(IndexError::BadLayout);
        }
        previous = Some(key);
        expected_offset = expected_offset
            .checked_add(entry.len)
            .ok_or(IndexError::BadLayout)?;
        table.push(entry);
    }
    if body_at
        .checked_add(u64::from(expected_offset))
        .ok_or(IndexError::BadLayout)?
        != file_len
    {
        return Err(IndexError::BadLayout);
    }
    Ok((expected_checksum, table, body_at, file_len))
}

fn validate_checksum(
    reader: &mut (impl Read + Seek),
    expected: u32,
    file_len: u64,
) -> Result<(), IndexError> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_error| IndexError::Truncated)?;
    let mut prefix = [0_u8; CHECKSUM_AT];
    reader
        .read_exact(&mut prefix)
        .map_err(|_error| IndexError::Truncated)?;
    reader
        .seek(SeekFrom::Start(HEADER_LEN as u64))
        .map_err(|_error| IndexError::Truncated)?;
    let mut checksum = Crc32c::new();
    checksum.update(&prefix);
    let mut remaining = file_len.saturating_sub(HEADER_LEN as u64);
    let mut buffer = [0_u8; CHECKSUM_CHUNK];
    while remaining != 0 {
        let take = usize::try_from(remaining.min(CHECKSUM_CHUNK as u64))
            .map_err(|_overflow| IndexError::TooLarge)?;
        reader
            .read_exact(&mut buffer[..take])
            .map_err(|_error| IndexError::Truncated)?;
        checksum.update(&buffer[..take]);
        remaining -= take as u64;
    }
    if checksum.finalize() == expected {
        Ok(())
    } else {
        Err(IndexError::BadChecksum)
    }
}

fn put_u32_at(bytes: &mut [u8], at: usize, value: u32) -> Result<(), IndexError> {
    bytes
        .get_mut(at..at.checked_add(4).ok_or(IndexError::BadLayout)?)
        .ok_or(IndexError::BadLayout)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

const fn checksum(head: &[u8], rest: &[u8]) -> u32 {
    let mut checksum = Crc32c::new();
    checksum.update(head);
    checksum.update(rest);
    checksum.finalize()
}

#[cfg(test)]
mod tests;
