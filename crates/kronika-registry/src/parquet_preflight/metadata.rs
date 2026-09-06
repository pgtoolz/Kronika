//! Validating the file metadata and page headers a section may carry.

use super::thrift_input::BoundedCompactInput;
use super::{
    CodecError, Encoding, FileMetaData, MAGIC, MAX_LIST_I32_VALUES_PER_SECTION, MAX_ROW_GROUPS,
    MAX_SECTION_ROWS, PageHeader, PageType, ParquetDecodeProfile, TSerializable,
};
use crate::codec::row_count_fits;

pub(super) fn parse_footer(body: &[u8]) -> Result<(FileMetaData, usize), CodecError> {
    if body.len() < 12 || body.get(..4) != Some(MAGIC) || body.get(body.len() - 4..) != Some(MAGIC)
    {
        return Err(CodecError::InvalidPageLayout);
    }
    let footer_len = u32::from_le_bytes(
        body[body.len() - 8..body.len() - 4]
            .try_into()
            .map_err(|_error| CodecError::InvalidPageLayout)?,
    ) as usize;
    let metadata_end = body.len() - 8;
    let metadata_start = metadata_end
        .checked_sub(footer_len)
        .filter(|&start| start >= MAGIC.len())
        .ok_or(CodecError::InvalidPageLayout)?;
    let mut protocol = BoundedCompactInput::new(&body[metadata_start..metadata_end]);
    let metadata = FileMetaData::read_from_in_protocol(&mut protocol)
        .map_err(|_error| CodecError::InvalidPageLayout)?;
    if protocol.remaining_len() != 0 || protocol.nesting != 0 {
        return Err(CodecError::InvalidPageLayout);
    }
    Ok((metadata, metadata_start))
}

#[allow(
    clippy::too_many_lines,
    reason = "the single pass keeps all cross-column footer and page accounting visible"
)]
pub(super) fn validate_file_metadata(
    body: &[u8],
    metadata: &FileMetaData,
    metadata_start: usize,
    max_decoded_bytes: usize,
    allow_dictionary: bool,
) -> Result<ParquetDecodeProfile, CodecError> {
    let rows = checked_rows(metadata.num_rows)?;
    if !row_count_fits(rows) {
        return Err(CodecError::TooManyRows {
            rows,
            max: MAX_SECTION_ROWS,
        });
    }
    if metadata.row_groups.len() > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups: metadata.row_groups.len(),
            max: MAX_ROW_GROUPS,
        });
    }
    if metadata.schema.is_empty()
        || metadata.encryption_algorithm.is_some()
        || metadata.footer_signing_key_metadata.is_some()
    {
        return Err(CodecError::InvalidPageLayout);
    }

    let mut grouped_rows = 0_usize;
    let mut footer_work = 0_usize;
    let mut ranges = Vec::new();
    for group in &metadata.row_groups {
        let group_rows = checked_rows(group.num_rows)?;
        if !row_count_fits(group_rows) {
            return Err(CodecError::TooManyRows {
                rows: group_rows,
                max: MAX_SECTION_ROWS,
            });
        }
        grouped_rows = grouped_rows
            .checked_add(group_rows)
            .ok_or(CodecError::InvalidPageLayout)?;
        if group.columns.is_empty() {
            return Err(CodecError::InvalidPageLayout);
        }
        for chunk in &group.columns {
            if chunk.file_path.is_some()
                || chunk.crypto_metadata.is_some()
                || chunk.encrypted_column_metadata.is_some()
            {
                return Err(CodecError::InvalidPageLayout);
            }
            let column = chunk
                .meta_data
                .as_ref()
                .ok_or(CodecError::InvalidPageLayout)?;
            let declared = checked_nonnegative(column.total_uncompressed_size)?;
            footer_work = add_work(footer_work, declared, max_decoded_bytes)?;
            let values = checked_nonnegative(column.num_values)?;
            if values > page_value_limit() {
                return Err(CodecError::InvalidPageLayout);
            }
            let compressed = checked_nonnegative(column.total_compressed_size)?;
            let data_start = checked_nonnegative(column.data_page_offset)?;
            let has_dictionary = column.dictionary_page_offset.is_some();
            if has_dictionary && !allow_dictionary {
                return Err(CodecError::DictionaryEncodingUnsupported);
            }
            let start = match column.dictionary_page_offset {
                Some(raw) => {
                    let dictionary_start = checked_nonnegative(raw)?;
                    if dictionary_start > data_start {
                        return Err(CodecError::InvalidPageLayout);
                    }
                    dictionary_start
                }
                None => data_start,
            };
            if compressed == 0 {
                if declared != 0
                    || values != 0
                    || has_dictionary
                    || start != data_start
                    || start < MAGIC.len()
                    || start > metadata_start
                {
                    return Err(CodecError::InvalidPageLayout);
                }
                continue;
            }
            let end = start
                .checked_add(compressed)
                .filter(|&end| {
                    start >= MAGIC.len()
                        && data_start >= start
                        && data_start <= end
                        && end <= metadata_start
                })
                .ok_or(CodecError::InvalidPageLayout)?;
            ranges.push((start, end, data_start, declared, values, has_dictionary));
        }
    }
    if grouped_rows != rows {
        return Err(CodecError::InvalidPageLayout);
    }

    ranges.sort_unstable_by_key(|&(start, _, _, _, _, _)| start);
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(CodecError::InvalidPageLayout);
    }

    let mut page_work = 0_usize;
    let mut page_count = 0_usize;
    for (start, end, data_start, declared, values, has_dictionary) in ranges {
        let mut offset = start;
        let mut column_work = 0_usize;
        let mut data_values = 0_usize;
        let mut saw_dictionary = false;
        let mut saw_data = false;
        while offset < end {
            page_count = page_count
                .checked_add(1)
                .filter(|&count| row_count_fits(count))
                .ok_or(CodecError::InvalidPageLayout)?;
            let mut protocol = BoundedCompactInput::new(&body[offset..end]);
            let before = protocol.remaining_len();
            let header = PageHeader::read_from_in_protocol(&mut protocol)
                .map_err(|_error| CodecError::InvalidPageLayout)?;
            let header_len = before
                .checked_sub(protocol.remaining_len())
                .filter(|&len| len != 0 && protocol.nesting == 0)
                .ok_or(CodecError::InvalidPageLayout)?;
            let compressed = checked_nonnegative(i64::from(header.compressed_page_size))?;
            let uncompressed = checked_nonnegative(i64::from(header.uncompressed_page_size))?;
            match header.type_ {
                PageType::DICTIONARY_PAGE
                    if has_dictionary && !saw_dictionary && offset == start => {}
                PageType::DICTIONARY_PAGE => return Err(CodecError::InvalidPageLayout),
                PageType::DATA_PAGE | PageType::DATA_PAGE_V2 if !saw_data => {
                    if offset != data_start {
                        return Err(CodecError::InvalidPageLayout);
                    }
                    saw_data = true;
                }
                PageType::DATA_PAGE | PageType::DATA_PAGE_V2 => {}
                _ if offset < data_start => return Err(CodecError::InvalidPageLayout),
                _ => {}
            }
            let decoded =
                header_len
                    .checked_add(uncompressed)
                    .ok_or(CodecError::DecodedSectionTooLarge {
                        len: usize::MAX,
                        max: max_decoded_bytes,
                    })?;
            column_work =
                column_work
                    .checked_add(decoded)
                    .ok_or(CodecError::DecodedSectionTooLarge {
                        len: usize::MAX,
                        max: max_decoded_bytes,
                    })?;
            page_work = add_work(page_work, decoded, max_decoded_bytes)?;
            validate_page_header(
                &header,
                uncompressed,
                compressed,
                &mut data_values,
                &mut saw_dictionary,
                allow_dictionary,
            )?;
            offset = offset
                .checked_add(header_len)
                .and_then(|offset| offset.checked_add(compressed))
                .filter(|&offset| offset <= end)
                .ok_or(CodecError::InvalidPageLayout)?;
        }
        if offset != end
            || column_work != declared
            || data_values != values
            || saw_dictionary != has_dictionary
            || (values != 0 && !saw_data)
        {
            return Err(CodecError::InvalidPageLayout);
        }
    }
    if page_work != footer_work {
        return Err(CodecError::InvalidPageLayout);
    }
    Ok(ParquetDecodeProfile {
        rows,
        decoded_bytes: page_work,
    })
}

pub(super) fn validate_data_encoding(
    encoding: Encoding,
    allow_dictionary: bool,
) -> Result<(), CodecError> {
    let admitted = encoding == Encoding::PLAIN
        || encoding == Encoding::RLE
        || (allow_dictionary
            && (encoding == Encoding::PLAIN_DICTIONARY || encoding == Encoding::RLE_DICTIONARY));
    if admitted {
        Ok(())
    } else {
        Err(CodecError::UnsupportedPageEncoding {
            encoding: encoding.0,
        })
    }
}

pub(super) fn validate_level_encoding(encoding: Encoding) -> Result<(), CodecError> {
    if encoding == Encoding::RLE || encoding == Encoding::BIT_PACKED {
        Ok(())
    } else {
        Err(CodecError::UnsupportedPageEncoding {
            encoding: encoding.0,
        })
    }
}

pub(super) fn validate_page_header(
    header: &PageHeader,
    uncompressed: usize,
    compressed: usize,
    data_values: &mut usize,
    saw_dictionary: &mut bool,
    allow_dictionary: bool,
) -> Result<(), CodecError> {
    match header.type_ {
        PageType::DATA_PAGE => {
            let page = header
                .data_page_header
                .as_ref()
                .filter(|_| {
                    header.data_page_header_v2.is_none()
                        && header.dictionary_page_header.is_none()
                        && header.index_page_header.is_none()
                })
                .ok_or(CodecError::InvalidPageLayout)?;
            validate_data_encoding(page.encoding, allow_dictionary)?;
            validate_level_encoding(page.definition_level_encoding)?;
            validate_level_encoding(page.repetition_level_encoding)?;
            add_page_values(data_values, page.num_values)?;
        }
        PageType::DATA_PAGE_V2 => {
            let page = header
                .data_page_header_v2
                .as_ref()
                .filter(|_| {
                    header.data_page_header.is_none()
                        && header.dictionary_page_header.is_none()
                        && header.index_page_header.is_none()
                })
                .ok_or(CodecError::InvalidPageLayout)?;
            validate_data_encoding(page.encoding, allow_dictionary)?;
            let values = checked_nonnegative(i64::from(page.num_values))?;
            let nulls = checked_nonnegative(i64::from(page.num_nulls))?;
            let rows = checked_nonnegative(i64::from(page.num_rows))?;
            let definition = checked_nonnegative(i64::from(page.definition_levels_byte_length))?;
            let repetition = checked_nonnegative(i64::from(page.repetition_levels_byte_length))?;
            if nulls > values || !row_count_fits(rows) {
                return Err(CodecError::InvalidPageLayout);
            }
            let levels = definition
                .checked_add(repetition)
                .filter(|&len| len <= uncompressed && len <= compressed)
                .ok_or(CodecError::InvalidPageLayout)?;
            let _ = levels;
            add_page_values(data_values, page.num_values)?;
        }
        PageType::DICTIONARY_PAGE => {
            let page = header
                .dictionary_page_header
                .as_ref()
                .filter(|_| {
                    header.data_page_header.is_none()
                        && header.data_page_header_v2.is_none()
                        && header.index_page_header.is_none()
                })
                .ok_or(CodecError::InvalidPageLayout)?;
            if page.encoding != Encoding::PLAIN && page.encoding != Encoding::PLAIN_DICTIONARY {
                return Err(CodecError::UnsupportedPageEncoding {
                    encoding: page.encoding.0,
                });
            }
            let values = checked_nonnegative(i64::from(page.num_values))?;
            if *saw_dictionary || values > page_value_limit() {
                return Err(CodecError::InvalidPageLayout);
            }
            *saw_dictionary = true;
        }
        PageType::INDEX_PAGE => {
            if header.index_page_header.is_none()
                || header.data_page_header.is_some()
                || header.data_page_header_v2.is_some()
                || header.dictionary_page_header.is_some()
            {
                return Err(CodecError::InvalidPageLayout);
            }
        }
        _ => return Err(CodecError::InvalidPageLayout),
    }
    Ok(())
}

pub(super) fn checked_rows(raw: i64) -> Result<usize, CodecError> {
    usize::try_from(raw).map_err(|_error| CodecError::InvalidRowCount { raw })
}

pub(super) fn checked_nonnegative(raw: i64) -> Result<usize, CodecError> {
    usize::try_from(raw).map_err(|_error| CodecError::InvalidPageLayout)
}

pub(super) const fn page_value_limit() -> usize {
    MAX_SECTION_ROWS.saturating_add(MAX_LIST_I32_VALUES_PER_SECTION)
}

pub(super) fn add_page_values(total: &mut usize, raw: i32) -> Result<(), CodecError> {
    let values = checked_nonnegative(i64::from(raw))?;
    *total = total
        .checked_add(values)
        .filter(|&count| count <= page_value_limit())
        .ok_or(CodecError::InvalidPageLayout)?;
    Ok(())
}

pub(super) fn add_work(total: usize, add: usize, max: usize) -> Result<usize, CodecError> {
    let next = total
        .checked_add(add)
        .ok_or(CodecError::DecodedSectionTooLarge {
            len: usize::MAX,
            max,
        })?;
    if next > max {
        Err(CodecError::DecodedSectionTooLarge { len: next, max })
    } else {
        Ok(next)
    }
}
