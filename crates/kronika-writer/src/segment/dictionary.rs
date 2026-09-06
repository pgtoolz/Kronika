//! Normalizing the per-window dictionaries into the segment's single pair.

use super::error::WriteError;
use super::{
    Array, ArrowReaderOptions, BTreeMap, BinaryArray, BooleanArray, CodecError, DICT_BLOBS_TYPE_ID,
    DICT_STRINGS_TYPE_ID, DataType, EntrySnapshot, Field, FinishedSection, FixedSizeBinaryArray,
    HotMark, Journal, MAX_DECODED_SECTION_BYTES, MAX_ROW_GROUPS, MAX_SECTION_BYTES,
    MAX_SECTION_ROWS, ParquetRecordBatchReaderBuilder, Placement, RecordBatch, Resolved, Schema,
    SectionDescriptor, StrId, UInt64Array, read_verified_body, validate_plain_parquet_decode_work,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum DictionaryValue {
    String(Vec<u8>),
    Blob {
        bytes: Vec<u8>,
        full_len: u64,
        truncated: bool,
        full_sha256: Option<[u8; 32]>,
    },
}

/// Canonical dictionary entries retained for one finished ZMS.
#[derive(Debug, Default)]
pub struct FinishedDictionary {
    values: BTreeMap<StrId, DictionaryValue>,
    string_rows: usize,
    blob_rows: usize,
    string_bytes: usize,
    blob_bytes: usize,
}

impl FinishedDictionary {
    /// Add one preserved string or blob representation.
    ///
    /// Repeating an identical entry is accepted. Reusing an id with different
    /// bytes, placement, or blob metadata is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when the representation is malformed,
    /// conflicts with an earlier entry, or exceeds a finished dictionary cap.
    pub fn insert(&mut self, str_id: StrId, resolved: Resolved<'_>) -> Result<(), WriteError> {
        let value = match resolved {
            Resolved::Str(bytes) => {
                if StrId::of(bytes) != Some(str_id) {
                    return Err(CodecError::SchemaMismatch.into());
                }
                if let Some(existing) = self.values.get(&str_id) {
                    return if matches!(existing, DictionaryValue::String(stored) if stored == bytes)
                    {
                        Ok(())
                    } else {
                        Err(WriteError::DictionaryConflict {
                            str_id: str_id.get(),
                        })
                    };
                }
                DictionaryValue::String(bytes.to_vec())
            }
            Resolved::Blob(blob) => {
                if blob.str_id != str_id {
                    return Err(CodecError::SchemaMismatch.into());
                }
                let valid = if blob.truncated {
                    (blob.stored_bytes.len() as u64) < blob.full_len && blob.full_sha256.is_some()
                } else {
                    blob.stored_bytes.len() as u64 == blob.full_len
                        && blob.full_sha256.is_none()
                        && StrId::of(blob.stored_bytes) == Some(str_id)
                };
                if !valid {
                    return Err(CodecError::SchemaMismatch.into());
                }
                if let Some(existing) = self.values.get(&str_id) {
                    return if matches!(
                        existing,
                        DictionaryValue::Blob {
                            bytes,
                            full_len,
                            truncated,
                            full_sha256,
                        } if bytes == blob.stored_bytes
                            && *full_len == blob.full_len
                            && *truncated == blob.truncated
                            && *full_sha256 == blob.full_sha256
                    ) {
                        Ok(())
                    } else {
                        Err(WriteError::DictionaryConflict {
                            str_id: str_id.get(),
                        })
                    };
                }
                DictionaryValue::Blob {
                    bytes: blob.stored_bytes.to_vec(),
                    full_len: blob.full_len,
                    truncated: blob.truncated,
                    full_sha256: blob.full_sha256,
                }
            }
        };
        self.insert_value(str_id, value)
    }

    /// Add an already-owned string representation without copying its bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when the id does not match the bytes, conflicts
    /// with an earlier entry, or exceeds a finished dictionary cap.
    pub fn insert_owned_string(&mut self, str_id: StrId, bytes: Vec<u8>) -> Result<(), WriteError> {
        if StrId::of(&bytes) != Some(str_id) {
            return Err(CodecError::SchemaMismatch.into());
        }
        self.insert_value(str_id, DictionaryValue::String(bytes))
    }

    /// Add an already-owned blob representation without copying its bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when the blob representation is malformed,
    /// conflicts with an earlier entry, or exceeds a finished dictionary cap.
    pub fn insert_owned_blob(
        &mut self,
        str_id: StrId,
        bytes: Vec<u8>,
        full_len: u64,
        truncated: bool,
        full_sha256: Option<[u8; 32]>,
    ) -> Result<(), WriteError> {
        let valid = if truncated {
            (bytes.len() as u64) < full_len && full_sha256.is_some()
        } else {
            bytes.len() as u64 == full_len
                && full_sha256.is_none()
                && StrId::of(&bytes) == Some(str_id)
        };
        if !valid {
            return Err(CodecError::SchemaMismatch.into());
        }
        self.insert_value(
            str_id,
            DictionaryValue::Blob {
                bytes,
                full_len,
                truncated,
                full_sha256,
            },
        )
    }

    fn insert_value(&mut self, str_id: StrId, value: DictionaryValue) -> Result<(), WriteError> {
        match self.values.get(&str_id) {
            Some(existing) if existing == &value => return Ok(()),
            Some(_) => {
                return Err(WriteError::DictionaryConflict {
                    str_id: str_id.get(),
                });
            }
            None => {}
        }
        let (rows, stored_bytes, value_bytes) = match &value {
            DictionaryValue::String(bytes) => {
                (&mut self.string_rows, &mut self.string_bytes, bytes.len())
            }
            DictionaryValue::Blob { bytes, .. } => {
                (&mut self.blob_rows, &mut self.blob_bytes, bytes.len())
            }
        };
        let next_rows = rows.checked_add(1).ok_or(WriteError::ArithmeticOverflow {
            what: "dictionary row count",
        })?;
        if next_rows > MAX_SECTION_ROWS {
            return Err(CodecError::TooManyRows {
                rows: next_rows,
                max: MAX_SECTION_ROWS,
            }
            .into());
        }
        let next_bytes =
            stored_bytes
                .checked_add(value_bytes)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "dictionary stored bytes",
                })?;
        if next_bytes > MAX_SECTION_BYTES {
            return Err(CodecError::SectionTooLarge {
                len: next_bytes,
                max: MAX_SECTION_BYTES,
            }
            .into());
        }
        *rows = next_rows;
        *stored_bytes = next_bytes;
        self.values.insert(str_id, value);
        Ok(())
    }

    /// Append canonical finished dictionary bodies to a spool.
    ///
    /// Returned descriptors begin at `offset` and can be combined with data
    /// descriptors in [`super::FinishedZmsPlan`].
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when encoding fails or offsets overflow.
    pub fn write_sections_to(
        &self,
        out: &mut (impl std::io::Write + Send),
        mut offset: u64,
    ) -> Result<Vec<FinishedSection>, WriteError> {
        let snapshots = self.values.iter().map(|(&str_id, value)| {
            let (stored_bytes, full_len, truncated, full_sha256, placement) = match value {
                DictionaryValue::String(bytes) => (
                    bytes.as_slice(),
                    bytes.len() as u64,
                    false,
                    None,
                    Placement::Strings,
                ),
                DictionaryValue::Blob {
                    bytes,
                    full_len,
                    truncated,
                    full_sha256,
                } => (
                    bytes.as_slice(),
                    *full_len,
                    *truncated,
                    *full_sha256,
                    Placement::Blobs,
                ),
            };
            EntrySnapshot {
                str_id,
                stored_bytes,
                full_len,
                truncated,
                full_sha256,
                placement,
                hot: HotMark::None,
                blob_required: placement == Placement::Blobs,
            }
        });
        let encoded =
            crate::dict::encode_final_entries_to(snapshots, out).map_err(WriteError::Codec)?;
        let mut sections = Vec::with_capacity(encoded.len());
        for section in encoded {
            let finished = FinishedSection::new(
                section.type_id,
                section.rows,
                offset,
                section.len,
                section.crc32c,
            )?;
            offset = offset
                .checked_add(section.len)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "dictionary spool offset",
                })?;
            sections.push(finished);
        }
        Ok(sections)
    }
}

pub(super) fn normalize_dictionary(
    journal: &Journal,
    strings: &[SectionDescriptor],
    blobs: &[SectionDescriptor],
) -> Result<FinishedDictionary, WriteError> {
    let mut normalized = FinishedDictionary::default();
    for &descriptor in strings.iter().chain(blobs) {
        decode_dictionary_body(journal, descriptor, &mut normalized)?;
    }
    Ok(normalized)
}

#[allow(
    clippy::too_many_lines,
    reason = "one pass validates ordering, schema, hashes, and blob metadata"
)]
pub(super) fn decode_dictionary_body(
    journal: &Journal,
    descriptor: SectionDescriptor,
    normalized: &mut FinishedDictionary,
) -> Result<(), WriteError> {
    let type_id = descriptor.entry.type_id;
    let is_blob = match type_id {
        DICT_STRINGS_TYPE_ID => false,
        DICT_BLOBS_TYPE_ID => true,
        _ => return Err(CodecError::UnknownType { type_id }.into()),
    };
    let body = read_verified_body(journal, descriptor)?.into_bytes();
    validate_plain_parquet_decode_work(body.as_ref(), MAX_DECODED_SECTION_BYTES)?;
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    let builder = ParquetRecordBatchReaderBuilder::try_new_with_options(body, options)?;
    let groups = builder.metadata().num_row_groups();
    if groups > MAX_ROW_GROUPS {
        return Err(CodecError::TooManyRowGroups {
            groups,
            max: MAX_ROW_GROUPS,
        }
        .into());
    }
    let claimed = builder.metadata().file_metadata().num_rows();
    let claimed_rows = match usize::try_from(claimed) {
        Ok(rows) if rows <= MAX_SECTION_ROWS => rows,
        Ok(rows) => {
            return Err(CodecError::TooManyRows {
                rows,
                max: MAX_SECTION_ROWS,
            }
            .into());
        }
        Err(_) => return Err(CodecError::InvalidRowCount { raw: claimed }.into()),
    };
    if claimed_rows != descriptor.entry.rows as usize {
        return Err(WriteError::RowCountMismatch {
            type_id,
            declared: descriptor.entry.rows,
            decoded: claimed_rows,
        });
    }
    if !dictionary_schema_matches(builder.schema(), is_blob) {
        return Err(CodecError::SchemaMismatch.into());
    }

    let mut previous = 0_u64;
    let mut decoded_rows = 0_usize;
    for batch in builder.with_batch_size(4_096).build()? {
        let batch = batch?;
        decoded_rows =
            decoded_rows
                .checked_add(batch.num_rows())
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "dictionary row count",
                })?;
        let ids = required_u64(&batch, "str_id")?;
        if is_blob {
            let bytes = required_binary(&batch, "stored_bytes")?;
            let full_len = required_u64(&batch, "full_len")?;
            let truncated = required_bool(&batch, "truncated")?;
            let full_sha256 = fixed_binary(&batch, "full_sha256")?;
            for row in 0..batch.num_rows() {
                let str_id = ordered_str_id(ids.value(row), &mut previous)?;
                let stored = bytes.value(row);
                let full_len = full_len.value(row);
                let truncated = truncated.value(row);
                let full_sha256 = if full_sha256.is_null(row) {
                    None
                } else {
                    Some(
                        full_sha256
                            .value(row)
                            .try_into()
                            .map_err(|_error| CodecError::SchemaMismatch)?,
                    )
                };
                let valid = if truncated {
                    (stored.len() as u64) < full_len && full_sha256.is_some()
                } else {
                    stored.len() as u64 == full_len
                        && full_sha256.is_none()
                        && StrId::of(stored) == Some(str_id)
                };
                if !valid {
                    return Err(CodecError::SchemaMismatch.into());
                }
                normalized.insert_value(
                    str_id,
                    DictionaryValue::Blob {
                        bytes: stored.to_vec(),
                        full_len,
                        truncated,
                        full_sha256,
                    },
                )?;
            }
        } else {
            let bytes = required_binary(&batch, "bytes")?;
            for row in 0..batch.num_rows() {
                let str_id = ordered_str_id(ids.value(row), &mut previous)?;
                let stored = bytes.value(row);
                if StrId::of(stored) != Some(str_id) {
                    return Err(CodecError::SchemaMismatch.into());
                }
                normalized.insert_value(str_id, DictionaryValue::String(stored.to_vec()))?;
            }
        }
    }
    if decoded_rows != claimed_rows {
        return Err(WriteError::RowCountMismatch {
            type_id,
            declared: descriptor.entry.rows,
            decoded: decoded_rows,
        });
    }
    Ok(())
}

pub(super) fn ordered_str_id(raw: u64, previous: &mut u64) -> Result<StrId, WriteError> {
    let str_id = StrId::from_raw(raw).ok_or(CodecError::SchemaMismatch)?;
    if raw <= *previous {
        return Err(CodecError::SchemaMismatch.into());
    }
    *previous = raw;
    Ok(str_id)
}

pub(super) fn dictionary_schema_matches(schema: &Schema, is_blob: bool) -> bool {
    let fields = schema.fields();
    if is_blob {
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
    } else {
        fields.len() == 2
            && field_matches(&fields[0], "str_id", &DataType::UInt64, false)
            && field_matches(&fields[1], "bytes", &DataType::Binary, false)
    }
}

pub(super) fn field_matches(
    field: &Field,
    name: &str,
    data_type: &DataType,
    nullable: bool,
) -> bool {
    field.name() == name && field.data_type() == data_type && field.is_nullable() == nullable
}

pub(super) fn required_u64<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a UInt64Array, CodecError> {
    let array = batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
        .ok_or(CodecError::ColumnType { name })?;
    reject_nulls(array, name).map(|()| array)
}

pub(super) fn required_binary<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a BinaryArray, CodecError> {
    let array = batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<BinaryArray>())
        .ok_or(CodecError::ColumnType { name })?;
    reject_nulls(array, name).map(|()| array)
}

pub(super) fn required_bool<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a BooleanArray, CodecError> {
    let array = batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<BooleanArray>())
        .ok_or(CodecError::ColumnType { name })?;
    reject_nulls(array, name).map(|()| array)
}

pub(super) fn fixed_binary<'a>(
    batch: &'a RecordBatch,
    name: &'static str,
) -> Result<&'a FixedSizeBinaryArray, CodecError> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<FixedSizeBinaryArray>())
        .ok_or(CodecError::ColumnType { name })
}

pub(super) fn reject_nulls(array: &dyn Array, name: &'static str) -> Result<(), CodecError> {
    if array.null_count() == 0 {
        Ok(())
    } else {
        Err(CodecError::NullInRequiredColumn { name })
    }
}
