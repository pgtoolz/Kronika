//! Committing a reset marker and recovering from a torn one.

use super::{
    File, FileExt, JOURNAL_HEADER_LEN, JournalConfig, JournalError, JournalHeader,
    RESET_MARKER_LEN, ResetMarker, Seek, SeekFrom, SegmentId, Write, map_scan_error,
    scan_journal_streaming_strict_from,
};
use crate::journal_failpoint;

pub(super) fn write_header(file: &mut File, header: JournalHeader) -> Result<(), std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header.encode())
}

pub(super) fn rollback(
    file: &mut File,
    end: usize,
    header: JournalHeader,
) -> Result<(), std::io::Error> {
    journal_failpoint!(RollbackTruncate);
    file.set_len(end as u64)?;
    journal_failpoint!(RollbackHeaderWrite);
    write_header(file, header)?;
    journal_failpoint!(RollbackSync);
    file.sync_data()
}

pub(super) fn finish_committed_reset(file: &mut File) -> Result<(), std::io::Error> {
    journal_failpoint!(ResetEmptyHeaderWrite);
    write_header(file, JournalHeader::EMPTY)?;
    journal_failpoint!(ResetEmptyHeaderSync);
    file.sync_data()?;
    journal_failpoint!(ResetTruncate);
    file.set_len(JOURNAL_HEADER_LEN as u64)?;
    journal_failpoint!(ResetFinalSync);
    file.sync_data()
}

pub(super) fn recover_committed_reset(
    file: &mut File,
    file_len: u64,
    config: JournalConfig,
) -> Result<bool, JournalError> {
    let minimum = (JOURNAL_HEADER_LEN + RESET_MARKER_LEN) as u64;
    if file_len < minimum {
        return Ok(false);
    }
    let marker_at = file_len - RESET_MARKER_LEN as u64;
    let mut bytes = [0_u8; RESET_MARKER_LEN];
    file.read_exact_at(&mut bytes, marker_at)?;
    let Some(marker) = ResetMarker::decode(bytes) else {
        return Ok(false);
    };
    if marker.previous_len != marker_at
        || marker.previous_len > u64::try_from(config.max_journal_len).unwrap_or(u64::MAX)
        || SegmentId::new(marker.previous_segment_id).is_err()
    {
        return Ok(false);
    }
    let Some(_expected_header) = marker.expected_previous_header() else {
        return Ok(false);
    };
    let mut header_bytes = [0_u8; JOURNAL_HEADER_LEN];
    file.read_exact_at(&mut header_bytes, 0)?;
    if marker.classify_header_transition(header_bytes).is_none() {
        return Ok(false);
    }
    let previous = PrefixReader {
        file,
        len: marker.previous_len,
    };
    let scan = scan_journal_streaming_strict_from(
        &previous,
        JOURNAL_HEADER_LEN as u64,
        config.limits,
        config.max_parts,
    )
    .map_err(map_scan_error)?;
    if scan.parts.is_empty()
        || u64::try_from(scan.valid_len).unwrap_or(u64::MAX) != marker.previous_len
    {
        return Ok(false);
    }
    finish_committed_reset(file)?;
    Ok(true)
}

pub(super) struct PrefixReader<'a> {
    file: &'a File,
    len: u64,
}

impl kronika_format::ReadAt for PrefixReader<'_> {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        let requested = u64::try_from(buf.len())
            .map_err(|_overflow| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
        if offset
            .checked_add(requested)
            .is_none_or(|end| end > self.len)
        {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        FileExt::read_exact_at(self.file, buf, offset)
    }

    fn byte_len(&self) -> std::io::Result<u64> {
        Ok(self.len)
    }
}
