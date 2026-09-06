//! Resolving the dictionary ids carried by row cells.

use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::ops::Range;

use arrow_array::{Array as _, BinaryArray, BooleanArray, FixedSizeBinaryArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use kronika_format::{BlobEntry, Resolved, StrId};
use kronika_registry::{
    Bytes, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, MAX_DECODED_SECTION_BYTES,
    VerifiedSection, plain_parquet_decode_profile,
};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowSelection,
};

/// One owned string or blob value decoded from a segment dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedDictionaryValue {
    /// UTF-8 is not assumed; dictionary strings retain their exact bytes.
    String(Vec<u8>),
    /// A full blob or its canonical truncated representation.
    Blob {
        /// Bytes physically stored in the dictionary.
        stored_bytes: Vec<u8>,
        /// Original byte length before optional truncation.
        full_len: u64,
        /// Whether only a prefix is stored.
        truncated: bool,
        /// Full-value digest, present exactly when truncated.
        full_sha256: Option<[u8; 32]>,
    },
}

/// One segment's complete `dict.strings` and `dict.blobs` dictionary.
#[derive(Debug, Default, Clone)]
pub struct Dictionary {
    by_id: HashMap<StrId, OwnedDictionaryValue>,
}

impl Dictionary {
    /// Resolve a raw [`crate::Cell::StrId`] value.
    #[must_use]
    pub fn resolve(&self, raw_id: u64) -> Option<Resolved<'_>> {
        let str_id = StrId::from_raw(raw_id)?;
        Some(resolved(str_id, self.by_id.get(&str_id)?))
    }

    /// Iterate all string and blob entries in unspecified order.
    pub fn entries(&self) -> impl Iterator<Item = (u64, Resolved<'_>)> + '_ {
        self.by_id
            .iter()
            .map(|(&str_id, value)| (str_id.get(), resolved(str_id, value)))
    }

    /// Consume the dictionary without copying its decoded value bytes.
    pub fn into_entries(self) -> impl Iterator<Item = (StrId, OwnedDictionaryValue)> {
        self.by_id.into_iter()
    }

    fn insert(&mut self, id: StrId, value: OwnedDictionaryValue) -> Result<(), CodecError> {
        match self.by_id.entry(id) {
            Entry::Vacant(slot) => {
                slot.insert(value);
                Ok(())
            }
            Entry::Occupied(slot) if slot.get() == &value => Ok(()),
            Entry::Occupied(_slot) => Err(CodecError::SchemaMismatch),
        }
    }

    pub(crate) fn decode(
        &mut self,
        type_id: u32,
        section: VerifiedSection,
        expected_rows: u64,
    ) -> Result<(), CodecError> {
        let (builder, _metadata) =
            dictionary_builder(section.into_bytes(), type_id, expected_rows)?;
        let reader = builder.build()?;
        for batch in reader {
            let batch = batch?;
            self.materialize_batch(type_id, &batch, None)?;
        }
        Ok(())
    }

    pub(crate) fn decode_selected(
        &mut self,
        type_id: u32,
        section: VerifiedSection,
        wanted: &HashSet<u64>,
        expected_rows: u64,
    ) -> Result<(), CodecError> {
        if wanted.is_empty() {
            return Ok(());
        }
        let bytes = section.into_bytes();
        let (id_builder, metadata) = dictionary_builder(bytes.clone(), type_id, expected_rows)?;
        let total_rows = usize::try_from(id_builder.metadata().file_metadata().num_rows())
            .map_err(|_overflow| CodecError::SchemaMismatch)?;
        let id_mask = ProjectionMask::roots(id_builder.parquet_schema(), [0]);
        let id_reader = id_builder.with_projection(id_mask).build()?;
        let mut ranges: Vec<Range<usize>> = Vec::new();
        let mut ordinal = 0_usize;
        for batch in id_reader {
            let batch = batch?;
            let ids = required_array::<UInt64Array>(&batch, "str_id")?;
            for row in 0..batch.num_rows() {
                if wanted.contains(&ids.value(row)) {
                    ranges.push(ordinal..ordinal.saturating_add(1));
                }
                ordinal = ordinal.saturating_add(1);
            }
        }
        if ranges.is_empty() {
            return Ok(());
        }
        let selection = RowSelection::from_consecutive_ranges(ranges.into_iter(), total_rows);
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(bytes, metadata)
            .with_row_selection(selection)
            .build()?;
        for batch in reader {
            let batch = batch?;
            self.materialize_batch(type_id, &batch, None)?;
        }
        Ok(())
    }

    pub(crate) fn decode_selected_once(
        &mut self,
        type_id: u32,
        section: VerifiedSection,
        wanted: &HashSet<u64>,
        expected_rows: u64,
    ) -> Result<(), CodecError> {
        if wanted.is_empty() {
            return Ok(());
        }
        let (builder, _metadata) =
            dictionary_builder(section.into_bytes(), type_id, expected_rows)?;
        let reader = builder.build()?;
        for batch in reader {
            let batch = batch?;
            self.materialize_batch(type_id, &batch, Some(wanted))?;
        }
        Ok(())
    }

    fn materialize_batch(
        &mut self,
        type_id: u32,
        batch: &arrow_array::RecordBatch,
        wanted: Option<&HashSet<u64>>,
    ) -> Result<(), CodecError> {
        let ids = required_array::<UInt64Array>(batch, "str_id")?;
        match type_id {
            DICT_STRINGS_TYPE_ID => {
                let values = required_array::<BinaryArray>(batch, "bytes")?;
                for row in 0..batch.num_rows() {
                    if wanted.is_some_and(|wanted| !wanted.contains(&ids.value(row))) {
                        continue;
                    }
                    let id = StrId::from_raw(ids.value(row)).ok_or(CodecError::SchemaMismatch)?;
                    self.insert(id, OwnedDictionaryValue::String(values.value(row).to_vec()))?;
                }
            }
            DICT_BLOBS_TYPE_ID => {
                let stored = required_array::<BinaryArray>(batch, "stored_bytes")?;
                let full_len = required_array::<UInt64Array>(batch, "full_len")?;
                let truncated = required_array::<BooleanArray>(batch, "truncated")?;
                let hash = nullable_hash_array(batch)?;
                for row in 0..batch.num_rows() {
                    if wanted.is_some_and(|wanted| !wanted.contains(&ids.value(row))) {
                        continue;
                    }
                    let id = StrId::from_raw(ids.value(row)).ok_or(CodecError::SchemaMismatch)?;
                    let full_sha256 = if hash.is_null(row) {
                        None
                    } else {
                        Some(
                            hash.value(row)
                                .try_into()
                                .map_err(|_error| CodecError::SchemaMismatch)?,
                        )
                    };
                    self.insert(
                        id,
                        OwnedDictionaryValue::Blob {
                            stored_bytes: stored.value(row).to_vec(),
                            full_len: full_len.value(row),
                            truncated: truncated.value(row),
                            full_sha256,
                        },
                    )?;
                }
            }
            _ => return Err(CodecError::UnknownType { type_id }),
        }
        Ok(())
    }
}

pub(crate) fn match_prefix(
    type_id: u32,
    section: VerifiedSection,
    expected_rows: u64,
    prefix: &[u8],
    matches: &mut HashSet<u64>,
) -> Result<(), CodecError> {
    let (builder, _metadata) = dictionary_builder(section.into_bytes(), type_id, expected_rows)?;
    let projection = ProjectionMask::roots(builder.parquet_schema(), [0, 1]);
    let reader = builder.with_projection(projection).build()?;
    let value_column = match type_id {
        DICT_STRINGS_TYPE_ID => "bytes",
        DICT_BLOBS_TYPE_ID => "stored_bytes",
        _ => return Err(CodecError::UnknownType { type_id }),
    };
    for batch in reader {
        let batch = batch?;
        let ids = required_array::<UInt64Array>(&batch, "str_id")?;
        let values = required_array::<BinaryArray>(&batch, value_column)?;
        for row in 0..batch.num_rows() {
            let id = StrId::from_raw(ids.value(row)).ok_or(CodecError::SchemaMismatch)?;
            if values.value(row).starts_with(prefix) {
                matches.insert(id.get());
            }
        }
    }
    Ok(())
}

fn dictionary_builder(
    bytes: Bytes,
    type_id: u32,
    expected_rows: u64,
) -> Result<(ParquetRecordBatchReaderBuilder<Bytes>, ArrowReaderMetadata), CodecError> {
    let profile = plain_parquet_decode_profile(bytes.as_ref(), MAX_DECODED_SECTION_BYTES)?;
    let got = u64::try_from(profile.rows).map_err(|_overflow| CodecError::SchemaMismatch)?;
    if got != expected_rows {
        return Err(CodecError::RowCountMismatch {
            expected: expected_rows,
            got,
        });
    }
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let metadata = ArrowReaderMetadata::load(&bytes, options)?;
    let builder = ParquetRecordBatchReaderBuilder::new_with_metadata(bytes, metadata.clone());
    if dictionary_schema_matches(builder.schema(), type_id) {
        Ok((builder, metadata))
    } else {
        Err(CodecError::SchemaMismatch)
    }
}

fn dictionary_schema_matches(schema: &Schema, type_id: u32) -> bool {
    let fields = schema.fields();
    match type_id {
        DICT_STRINGS_TYPE_ID => {
            fields.len() == 2
                && field_matches(&fields[0], "str_id", &DataType::UInt64, false)
                && field_matches(&fields[1], "bytes", &DataType::Binary, false)
        }
        DICT_BLOBS_TYPE_ID => {
            fields.len() == 5
                && field_matches(&fields[0], "str_id", &DataType::UInt64, false)
                && field_matches(&fields[1], "stored_bytes", &DataType::Binary, false)
                && field_matches(&fields[2], "full_len", &DataType::UInt64, false)
                && field_matches(&fields[3], "truncated", &DataType::Boolean, false)
                && field_matches(
                    &fields[4],
                    "full_sha256",
                    &DataType::FixedSizeBinary(32),
                    true,
                )
        }
        _ => false,
    }
}

fn field_matches(field: &Field, name: &str, ty: &DataType, nullable: bool) -> bool {
    field.name() == name && field.data_type() == ty && field.is_nullable() == nullable
}

fn resolved(str_id: StrId, value: &OwnedDictionaryValue) -> Resolved<'_> {
    match value {
        OwnedDictionaryValue::String(bytes) => Resolved::Str(bytes),
        OwnedDictionaryValue::Blob {
            stored_bytes,
            full_len,
            truncated,
            full_sha256,
        } => Resolved::Blob(BlobEntry {
            str_id,
            stored_bytes,
            full_len: *full_len,
            truncated: *truncated,
            full_sha256: *full_sha256,
        }),
    }
}

fn required_array<'a, A: arrow_array::Array + 'static>(
    batch: &'a arrow_array::RecordBatch,
    name: &'static str,
) -> Result<&'a A, CodecError> {
    let array = batch
        .column_by_name(name)
        .ok_or(CodecError::MissingColumn { name })?
        .as_any()
        .downcast_ref::<A>()
        .ok_or(CodecError::ColumnType { name })?;
    if array.null_count() == 0 {
        Ok(array)
    } else {
        Err(CodecError::NullInRequiredColumn { name })
    }
}

fn nullable_hash_array(
    batch: &arrow_array::RecordBatch,
) -> Result<&FixedSizeBinaryArray, CodecError> {
    const NAME: &str = "full_sha256";
    let array = batch
        .column_by_name(NAME)
        .ok_or(CodecError::MissingColumn { name: NAME })?
        .as_any()
        .downcast_ref::<FixedSizeBinaryArray>()
        .ok_or(CodecError::ColumnType { name: NAME })?;
    if array.value_length() == 32 {
        Ok(array)
    } else {
        Err(CodecError::ColumnType { name: NAME })
    }
}

#[cfg(test)]
mod tests {
    use super::{Dictionary, OwnedDictionaryValue};
    use kronika_format::StrId;

    #[test]
    fn repeated_dictionary_id_requires_the_same_representation() {
        let id = StrId::from_raw(7).expect("nonzero id");
        let mut dictionary = Dictionary::default();
        dictionary
            .insert(id, OwnedDictionaryValue::String(b"same".to_vec()))
            .expect("first value");
        dictionary
            .insert(id, OwnedDictionaryValue::String(b"same".to_vec()))
            .expect("identical repeat");
        assert!(
            dictionary
                .insert(id, OwnedDictionaryValue::String(b"different".to_vec()),)
                .is_err(),
            "a changed value for one id must fail"
        );
    }

    #[test]
    fn repeated_dictionary_id_cannot_change_placement_or_blob_metadata() {
        let id = StrId::from_raw(8).expect("nonzero id");
        let blob = OwnedDictionaryValue::Blob {
            stored_bytes: b"prefix".to_vec(),
            full_len: 12,
            truncated: true,
            full_sha256: Some([3; 32]),
        };
        let mut dictionary = Dictionary::default();
        dictionary.insert(id, blob.clone()).expect("first blob");
        dictionary.insert(id, blob).expect("identical blob");
        assert!(
            dictionary
                .insert(
                    id,
                    OwnedDictionaryValue::Blob {
                        stored_bytes: b"prefix".to_vec(),
                        full_len: 13,
                        truncated: true,
                        full_sha256: Some([3; 32]),
                    },
                )
                .is_err(),
            "changed blob metadata must fail"
        );
        assert!(
            dictionary
                .insert(id, OwnedDictionaryValue::String(b"prefix".to_vec()))
                .is_err(),
            "string/blob placement changes must fail"
        );
    }
}
