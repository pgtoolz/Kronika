//! Validate section bounds and reuse Parquet footers while projecting columns.

use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader,
    ParquetRecordBatchReaderBuilder,
};
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use parquet::file::reader::ChunkReader;
use std::sync::Arc;

use super::MAX_CACHED_METADATA_BYTES;
use crate::codec::{
    CodecError, DECODE_BATCH_SIZE, MAX_DECODED_SECTION_BYTES, MAX_ROW_GROUPS, MAX_SECTION_BYTES,
    TypeContract, arrow_schema, check_row_cap, schema_matches,
};

pub(super) fn full_section_reader<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    expected_rows: usize,
    metadata: &mut SectionMetadataCache,
    section_index: usize,
) -> Result<ParquetRecordBatchReader, CodecError> {
    let builder = section_reader_builder(
        source,
        contract,
        expected_rows,
        metadata,
        section_index,
        true,
    )?;
    Ok(builder.with_batch_size(DECODE_BATCH_SIZE).build()?)
}

pub(super) struct SectionMetadataCache {
    sections: Vec<Option<Arc<ParquetMetaData>>>,
    validated: Vec<bool>,
    bytes: usize,
    enabled: bool,
}

impl SectionMetadataCache {
    pub(super) fn new(sections: usize) -> Self {
        Self {
            sections: (0..sections).map(|_index| None).collect(),
            validated: vec![false; sections],
            bytes: 0,
            enabled: true,
        }
    }

    fn validate<R: ChunkReader>(
        &mut self,
        section_index: usize,
        source: &R,
        len: usize,
    ) -> Result<(), CodecError> {
        let validated = self
            .validated
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if !*validated {
            let body = source.get_bytes(0, len)?;
            crate::validate_parquet_decode_work(body.as_ref(), MAX_DECODED_SECTION_BYTES)?;
            *validated = true;
        }
        Ok(())
    }

    fn get(
        &mut self,
        section_index: usize,
        remove: bool,
    ) -> Result<Option<Arc<ParquetMetaData>>, CodecError> {
        let section = self
            .sections
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if remove {
            let Some(metadata) = section.take() else {
                return Ok(None);
            };
            self.bytes = self
                .bytes
                .checked_sub(metadata.memory_size())
                .ok_or(CodecError::SchemaMismatch)?;
            Ok(Some(metadata))
        } else {
            Ok(section.as_ref().map(Arc::clone))
        }
    }

    fn remember(
        &mut self,
        section_index: usize,
        metadata: &Arc<ParquetMetaData>,
    ) -> Result<(), CodecError> {
        if !self.enabled {
            return Ok(());
        }
        let next_bytes = self
            .bytes
            .checked_add(metadata.memory_size())
            .ok_or(CodecError::SchemaMismatch)?;
        if next_bytes > MAX_CACHED_METADATA_BYTES {
            self.enabled = false;
            return Ok(());
        }
        let slot = self
            .sections
            .get_mut(section_index)
            .ok_or(CodecError::SchemaMismatch)?;
        if slot.is_some() {
            return Err(CodecError::SchemaMismatch);
        }
        *slot = Some(Arc::clone(metadata));
        self.bytes = next_bytes;
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        self.sections.fill(None);
        self.bytes = 0;
        self.enabled = false;
    }
}

pub(super) fn section_reader_builder<R: ChunkReader + 'static>(
    source: R,
    contract: &TypeContract,
    expected_rows: usize,
    cache: &mut SectionMetadataCache,
    section_index: usize,
    remove_metadata: bool,
) -> Result<ParquetRecordBatchReaderBuilder<R>, CodecError> {
    let len = usize::try_from(source.len()).map_err(|_overflow| CodecError::SectionTooLarge {
        len: usize::MAX,
        max: MAX_SECTION_BYTES,
    })?;
    if len > MAX_SECTION_BYTES {
        return Err(CodecError::SectionTooLarge {
            len,
            max: MAX_SECTION_BYTES,
        });
    }
    cache.validate(section_index, &source, len)?;
    let (metadata, cached) = if let Some(metadata) = cache.get(section_index, remove_metadata)? {
        (metadata, true)
    } else {
        let metadata = Arc::new(ParquetMetaDataReader::new().parse_and_finish(&source)?);
        validate_section_metadata(metadata.as_ref(), expected_rows)?;
        if !remove_metadata {
            cache.remember(section_index, &metadata)?;
        }
        (metadata, false)
    };
    let metadata = if cached {
        let options = ArrowReaderOptions::new().with_schema(arrow_schema(contract));
        ArrowReaderMetadata::try_new(metadata, options)?
    } else {
        let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
        let metadata = ArrowReaderMetadata::try_new(metadata, options)?;
        if !schema_matches(metadata.schema().as_ref(), contract) {
            return Err(CodecError::SchemaMismatch);
        }
        metadata
    };
    Ok(ParquetRecordBatchReaderBuilder::new_with_metadata(
        source, metadata,
    ))
}

fn validate_section_metadata(
    metadata: &ParquetMetaData,
    expected_rows: usize,
) -> Result<(), CodecError> {
    let groups = metadata.num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        });
    }
    let claimed = metadata.file_metadata().num_rows();
    let claimed = usize::try_from(claimed)
        .map_err(|_overflow| CodecError::InvalidRowCount { raw: claimed })?;
    check_row_cap(claimed)?;
    if claimed != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows as u64,
            got: claimed as u64,
        });
    }
    Ok(())
}
