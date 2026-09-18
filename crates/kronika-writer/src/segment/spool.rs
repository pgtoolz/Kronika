//! Coalesce journal sections into a bounded spool before final publication.

use std::cmp::Reverse;
use std::io::{self, BufWriter, Write};

use kronika_format::Crc32c;
use kronika_layout::ZmsTemp;
use kronika_registry::{
    Bytes, CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, encode_final_sections_to,
};

use super::plan::{
    SectionDescriptor, aggregate_rows, plan_segment, read_section_body, read_verified_body,
};
use super::{
    FinishedSection, FinishedZmsPlan, WriteError, WriteSummary, check_final_section_len,
    normalize_dictionary, write_finished_zms,
};
use crate::Journal;

/// Write the merged segment to `tmp` and flush the encoder.
///
/// Publication synchronizes the file and its parent directories.
pub(super) fn write_tmp(
    journal: &Journal,
    temporary: &mut ZmsTemp<'_>,
    spool: &mut ZmsTemp<'_>,
) -> Result<WriteSummary, WriteError> {
    let mut plan = plan_segment(journal)?;
    let strings = plan
        .by_type
        .remove(&DICT_STRINGS_TYPE_ID)
        .unwrap_or_default();
    let blobs = plan.by_type.remove(&DICT_BLOBS_TYPE_ID).unwrap_or_default();

    let mut types = plan.by_type.into_iter().collect::<Vec<_>>();
    types.sort_by_key(|(type_id, descriptors)| {
        let bytes = descriptors.iter().fold(0_u64, |total, descriptor| {
            total.saturating_add(descriptor.entry.len)
        });
        (Reverse(bytes), *type_id)
    });
    let mut spool_out = BufWriter::new(spool.file_mut());
    let mut spooled = Vec::new();
    let mut spool_offset = 0_u64;

    for (type_id, descriptors) in types {
        let section =
            spool_data_section(journal, type_id, &descriptors, &mut spool_out, spool_offset)?;
        spool_offset =
            spool_offset
                .checked_add(section.len)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "spool offset",
                })?;
        spooled.push(section);
    }

    spool_dictionary_sections(
        journal,
        &strings,
        &blobs,
        &mut spool_out,
        &mut spooled,
        &mut spool_offset,
    )?;
    let spool_file = spool_out
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?;
    let finished = FinishedZmsPlan::new(spooled, plan.min_ts, plan.max_ts, plan.window_count)?;
    let summary = write_finished_zms(spool_file, &finished, temporary.file_mut())?;
    temporary.file_mut().sync_all()?;
    Ok(summary)
}

fn spool_data_section(
    journal: &Journal,
    type_id: u32,
    descriptors: &[SectionDescriptor],
    out: &mut (impl Write + Send),
    offset: u64,
) -> Result<FinishedSection, WriteError> {
    let declared_rows = aggregate_rows(type_id, descriptors)?;
    let rows = descriptors
        .iter()
        .map(|descriptor| descriptor.entry.rows)
        .collect::<Vec<_>>();
    let mut verified = vec![false; descriptors.len()];
    let mut sink = SectionSink::new(out);
    encode_final_sections_to(
        type_id,
        &rows,
        &mut sink,
        |index| -> Result<_, WriteError> {
            let descriptor = descriptors
                .get(index)
                .copied()
                .ok_or(CodecError::SchemaMismatch)?;
            if verified[index] {
                Ok(Bytes::from(read_section_body(journal, descriptor)?))
            } else {
                verified[index] = true;
                Ok(read_verified_body(journal, descriptor)?.into_bytes())
            }
        },
    )?;
    let (len, checksum) = sink.finish();
    check_final_section_len(len)?;
    FinishedSection::new(
        type_id,
        u32::try_from(declared_rows).map_err(|_overflow| WriteError::ArithmeticOverflow {
            what: "section row count",
        })?,
        offset,
        len,
        checksum,
    )
}

fn spool_dictionary_sections(
    journal: &Journal,
    strings: &[SectionDescriptor],
    blobs: &[SectionDescriptor],
    out: &mut (impl Write + Send),
    spooled: &mut Vec<FinishedSection>,
    offset: &mut u64,
) -> Result<(), WriteError> {
    let dictionary = normalize_dictionary(journal, strings, blobs)?;
    for section in dictionary.write_sections_to(out, *offset)? {
        *offset =
            section
                .offset
                .checked_add(section.len)
                .ok_or(WriteError::ArithmeticOverflow {
                    what: "spool offset",
                })?;
        spooled.push(section);
    }
    Ok(())
}

struct SectionSink<W> {
    out: W,
    len: u64,
    checksum: Crc32c,
}

impl<W> SectionSink<W> {
    const fn new(out: W) -> Self {
        Self {
            out,
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
        let written = self.out.write(buf)?;
        self.checksum.update(&buf[..written]);
        self.len = self
            .len
            .checked_add(written as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "section length overflow"))?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
