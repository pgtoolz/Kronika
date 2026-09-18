//! Stage retained batches and their dictionaries, then encode final sections.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::FileExt as _;

use arrow_array::{Array as _, RecordBatch, UInt64Array};
use kronika_format::{Crc32c, StrId};
use kronika_reader::{OwnedDictionaryValue, Reader, SegmentRef};
use kronika_registry::{
    Bytes, ColumnType, TypeContract, encode_final_batches, encode_final_sections_to,
};
use kronika_writer::{FinishedDictionary, FinishedSection};

use super::selection::{TypeSelection, retain_batch};
use super::{MicrosRange, SliceError, is_dictionary};

#[derive(Debug)]
pub(super) struct StagedSelection {
    staged: BTreeMap<u32, Vec<StagedSection>>,
    dictionary: FinishedDictionary,
    staging_end: u64,
    pub(super) actual_min_ts: i64,
    pub(super) actual_max_ts: i64,
    pub(super) rows_written: u64,
}

pub(super) fn stage_selected_rows(
    reader: &Reader,
    references: &[SegmentRef],
    selections: &BTreeMap<u32, TypeSelection>,
    range: MicrosRange,
    staging: &mut File,
) -> Result<StagedSelection, SliceError> {
    let mut selected = StagedSelection {
        staged: BTreeMap::new(),
        dictionary: FinishedDictionary::default(),
        staging_end: 0,
        actual_min_ts: i64::MAX,
        actual_max_ts: i64::MIN,
        rows_written: 0,
    };
    for reference in references {
        let segment = reader.open_segment(reference)?;
        let mut dictionary_ids = HashSet::new();
        for type_id in segment.type_ids() {
            if is_dictionary(type_id) {
                continue;
            }
            let selection = selections
                .get(&type_id)
                .ok_or(SliceError::UnsliceableType { type_id })?;
            let mut callback_error = None;
            segment.visit_batches(
                type_id,
                None,
                0,
                usize::MAX,
                |_ordinal, batch| match selected.stage_batch(
                    staging,
                    &batch,
                    selection,
                    range,
                    &mut dictionary_ids,
                ) {
                    Ok(()) => true,
                    Err(problem) => {
                        callback_error = Some(problem);
                        false
                    }
                },
            )?;
            if let Some(problem) = callback_error {
                return Err(problem);
            }
        }
        if !dictionary_ids.is_empty() {
            let dictionary = segment.dictionary_for(&dictionary_ids)?;
            for (str_id, value) in dictionary.into_entries() {
                if !dictionary_ids.remove(&str_id.get()) {
                    continue;
                }
                match value {
                    OwnedDictionaryValue::String(bytes) => {
                        selected.dictionary.insert_owned_string(str_id, bytes)?;
                    }
                    OwnedDictionaryValue::Blob {
                        stored_bytes,
                        full_len,
                        truncated,
                        full_sha256,
                    } => selected.dictionary.insert_owned_blob(
                        str_id,
                        stored_bytes,
                        full_len,
                        truncated,
                        full_sha256,
                    )?,
                }
            }
            if let Some(&raw) = dictionary_ids.iter().next() {
                return Err(SliceError::UnresolvedDictionary { str_id: raw });
            }
        }
    }
    Ok(selected)
}

#[derive(Debug, Clone, Copy)]
struct StagedSection {
    offset: u64,
    len: u64,
    rows: u32,
}

fn collect_dictionary_ids(
    batch: &RecordBatch,
    contract: &TypeContract,
    ids: &mut HashSet<u64>,
) -> Result<(), SliceError> {
    for column in contract
        .columns
        .iter()
        .filter(|column| column.ty == ColumnType::StrId)
    {
        let values = batch
            .column_by_name(column.name)
            .and_then(|array| array.as_any().downcast_ref::<UInt64Array>())
            .ok_or(SliceError::InvalidBatch {
                type_id: contract.type_id.get(),
                column: column.name,
            })?;
        for row in 0..values.len() {
            if values.is_null(row) {
                continue;
            }
            let raw = values.value(row);
            StrId::from_raw(raw).ok_or(SliceError::UnresolvedDictionary { str_id: raw })?;
            ids.insert(raw);
        }
    }
    Ok(())
}

fn staged_body(staging: &File, staged: StagedSection) -> Result<Bytes, SliceError> {
    let len = usize::try_from(staged.len).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
    let mut body = vec![0_u8; len];
    staging.read_exact_at(&mut body, staged.offset)?;
    Ok(Bytes::from(body))
}

struct SectionSink<W> {
    output: W,
    len: u64,
    checksum: Crc32c,
}

impl<W> SectionSink<W> {
    const fn new(output: W) -> Self {
        Self {
            output,
            len: 0,
            checksum: Crc32c::new(),
        }
    }

    fn finish(self) -> (u64, u32) {
        (self.len, self.checksum.finalize())
    }
}

impl<W: Write> Write for SectionSink<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.output.write(buf)?;
        self.checksum.update(&buf[..written]);
        self.len = self
            .len
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::other("section length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

impl StagedSelection {
    fn stage_batch(
        &mut self,
        staging: &mut File,
        batch: &RecordBatch,
        selection: &TypeSelection,
        range: MicrosRange,
        dictionary_ids: &mut HashSet<u64>,
    ) -> Result<(), SliceError> {
        let Some((batch, min_ts, max_ts)) = retain_batch(batch, selection, range)? else {
            return Ok(());
        };
        let contract = selection.contract;
        collect_dictionary_ids(&batch, contract, dictionary_ids)?;
        let type_id = contract.type_id.get();
        let rows =
            u32::try_from(batch.num_rows()).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
        let body = encode_final_batches(type_id, vec![batch])?;
        let len = u64::try_from(body.len()).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
        let offset = self.staging_end;
        let next = offset
            .checked_add(len)
            .ok_or(SliceError::ArithmeticOverflow)?;
        staging.write_all(&body)?;
        self.staging_end = next;
        // Each type keeps encounter order; final encoding preserves equal-time rows.
        self.staged
            .entry(type_id)
            .or_default()
            .push(StagedSection { offset, len, rows });
        self.rows_written = self
            .rows_written
            .checked_add(u64::from(rows))
            .ok_or(SliceError::ArithmeticOverflow)?;
        self.actual_min_ts = self.actual_min_ts.min(min_ts);
        self.actual_max_ts = self.actual_max_ts.max(max_ts);
        Ok(())
    }

    /// Append final sections after the staged batches, using positional reads so
    /// the cloned descriptor cannot move the writer's append position.
    pub(super) fn finalize(&self, scratch: &mut File) -> Result<Vec<FinishedSection>, SliceError> {
        let staging = scratch.try_clone()?;
        let mut spool = BufWriter::new(scratch);
        let mut offset = self.staging_end;
        let mut sections = Vec::with_capacity(self.staged.len());
        for (&type_id, sources) in &self.staged {
            let rows = sources.iter().map(|source| source.rows).collect::<Vec<_>>();
            let total_rows = rows.iter().try_fold(0_u32, |total, rows| {
                total
                    .checked_add(*rows)
                    .ok_or(SliceError::ArithmeticOverflow)
            })?;
            let mut sink = SectionSink::new(&mut spool);
            encode_final_sections_to(type_id, &rows, &mut sink, |index| {
                let source = sources.get(index).ok_or(SliceError::ArithmeticOverflow)?;
                staged_body(&staging, *source)
            })?;
            let (len, checksum) = sink.finish();
            sections.push(FinishedSection::new(
                type_id, total_rows, offset, len, checksum,
            )?);
            offset = offset
                .checked_add(len)
                .ok_or(SliceError::ArithmeticOverflow)?;
        }
        sections.extend(self.dictionary.write_sections_to(&mut spool, offset)?);
        spool.flush()?;
        Ok(sections)
    }
}
