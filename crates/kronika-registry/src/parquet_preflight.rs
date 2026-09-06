//! Allocation-safe Parquet footer and page admission before Arrow decode.

use parquet::format::{Encoding, FileMetaData, PageHeader, PageType};
use parquet::thrift::TSerializable;
use thrift::protocol::{
    TFieldIdentifier, TInputProtocol, TListIdentifier, TMapIdentifier, TMessageIdentifier,
    TSetIdentifier, TStructIdentifier, TType,
};

use crate::codec::MAX_LIST_I32_VALUES_PER_SECTION;
use crate::{
    CodecError, MAX_DECODED_SECTION_BYTES, MAX_ROW_GROUPS, MAX_SECTION_BYTES, MAX_SECTION_ROWS,
    SECTION_WRITE_BATCH_ROWS,
};

mod metadata;
mod thrift_input;

use metadata::{parse_footer, validate_file_metadata};
use thrift_input::{BoundedCompactInput, compact_type, invalid_data};
const MAGIC: &[u8; 4] = b"PAR1";
const MAX_THRIFT_NESTING: usize = 16;
const MAX_THRIFT_ITEMS: usize = SECTION_WRITE_BATCH_ROWS;

/// Exact bounded work declared by a validated Parquet section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParquetDecodeProfile {
    /// Rows declared consistently by the file and its row groups.
    pub rows: usize,
    /// Page-header plus uncompressed page-payload bytes.
    pub decoded_bytes: usize,
}

/// Validate all allocation-driving Parquet metadata before Arrow sees it.
///
/// The footer and every page header are parsed directly from the bounded body.
/// Footer totals, page claims, value counts, and exact column ranges must agree.
///
/// # Errors
///
/// Returns a typed [`CodecError`] for an oversized body or decoded-work claim,
/// excessive rows or row groups, or an inconsistent Parquet layout.
pub fn validate_parquet_decode_work(
    body: &[u8],
    max_decoded_bytes: usize,
) -> Result<(), CodecError> {
    parquet_decode_profile(body, max_decoded_bytes).map(|_profile| ())
}

/// Validate a section and return its exact uncompressed Parquet work.
///
/// This performs the same bounded footer and page-header pass as
/// [`validate_parquet_decode_work`] without materializing Arrow arrays.
///
/// # Errors
///
/// Returns the same typed [`CodecError`] values as
/// [`validate_parquet_decode_work`].
pub fn parquet_decode_profile(
    body: &[u8],
    max_decoded_bytes: usize,
) -> Result<ParquetDecodeProfile, CodecError> {
    validate_parquet_decode_profile(body, max_decoded_bytes, true)
}

/// Validate Parquet decode work and reject dictionary pages before Arrow.
///
/// Variable-width dictionary sections use this profile because encoded page
/// sizes do not bound the bytes materialized when dictionary indices expand.
///
/// # Errors
///
/// Returns [`CodecError::DictionaryEncodingUnsupported`] when the footer
/// declares a dictionary page, [`CodecError::UnsupportedPageEncoding`] for a
/// page encoding outside PLAIN/RLE, or the same bounded-layout errors as
/// [`validate_parquet_decode_work`].
pub fn validate_plain_parquet_decode_work(
    body: &[u8],
    max_decoded_bytes: usize,
) -> Result<(), CodecError> {
    plain_parquet_decode_profile(body, max_decoded_bytes).map(|_profile| ())
}

/// Validate a dictionary-free section and return exact uncompressed work.
///
/// # Errors
///
/// Returns the same typed [`CodecError`] values as
/// [`validate_plain_parquet_decode_work`].
pub fn plain_parquet_decode_profile(
    body: &[u8],
    max_decoded_bytes: usize,
) -> Result<ParquetDecodeProfile, CodecError> {
    validate_parquet_decode_profile(body, max_decoded_bytes, false)
}

fn validate_parquet_decode_profile(
    body: &[u8],
    max_decoded_bytes: usize,
    allow_dictionary: bool,
) -> Result<ParquetDecodeProfile, CodecError> {
    if body.len() > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len: body.len(),
            max: MAX_SECTION_BYTES,
        });
    }
    if max_decoded_bytes > MAX_DECODED_SECTION_BYTES {
        return Err(CodecError::DecodedSectionTooLarge {
            len: max_decoded_bytes,
            max: MAX_DECODED_SECTION_BYTES,
        });
    }
    let (metadata, metadata_start) = parse_footer(body)?;
    validate_file_metadata(
        body,
        &metadata,
        metadata_start,
        max_decoded_bytes,
        allow_dictionary,
    )
}

impl<'a> BoundedCompactInput<'a> {
    const fn new(remaining: &'a [u8]) -> Self {
        Self {
            remaining,
            last_field_id: 0,
            field_stack: [0; MAX_THRIFT_NESTING],
            struct_depth: 0,
            nesting: 0,
            collection_items: 0,
            pending_bool: None,
        }
    }

    const fn remaining_len(&self) -> usize {
        self.remaining.len()
    }

    fn enter(&mut self) -> thrift::Result<()> {
        if self.nesting >= MAX_THRIFT_NESTING {
            return Err(invalid_data("compact metadata nesting is too deep"));
        }
        self.nesting += 1;
        Ok(())
    }

    fn leave(&mut self) -> thrift::Result<()> {
        self.nesting = self
            .nesting
            .checked_sub(1)
            .ok_or_else(|| invalid_data("unbalanced compact metadata"))?;
        Ok(())
    }

    fn admit_items(&mut self, count: usize) -> thrift::Result<()> {
        if count > self.remaining.len() {
            return Err(invalid_data("collection exceeds remaining input"));
        }
        self.collection_items = self
            .collection_items
            .checked_add(count)
            .filter(|&items| items <= MAX_THRIFT_ITEMS)
            .ok_or_else(|| invalid_data("compact metadata has too many collection items"))?;
        Ok(())
    }

    fn read_vlq(&mut self) -> thrift::Result<u64> {
        let mut value = 0_u64;
        for index in 0..10_u32 {
            let byte = self.read_byte()?;
            let payload = u64::from(byte & 0x7f);
            if index == 9 && payload > 1 {
                return Err(invalid_data("compact integer exceeds u64"));
            }
            value |= payload << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid_data("unterminated compact integer"))
    }

    fn read_zigzag(&mut self) -> thrift::Result<i64> {
        let value = self.read_vlq()?;
        let half = i64::try_from(value >> 1)
            .map_err(|_error| invalid_data("compact integer exceeds i64"))?;
        Ok(if value & 1 == 0 { half } else { !half })
    }

    fn read_collection(&mut self) -> thrift::Result<(TType, i32)> {
        self.enter()?;
        let header = self.read_byte()?;
        let element_type = compact_type(header & 0x0f)?;
        if element_type == TType::Stop {
            return Err(invalid_data("collection element type is stop"));
        }
        let inline = (header & 0xf0) >> 4;
        let count = if inline == 15 {
            self.read_vlq()?
        } else {
            u64::from(inline)
        };
        let count =
            usize::try_from(count).map_err(|_error| invalid_data("collection is too large"))?;
        self.admit_items(count)?;
        let count =
            i32::try_from(count).map_err(|_error| invalid_data("collection is too large"))?;
        Ok((element_type, count))
    }
}

#[cfg(test)]
mod tests;
