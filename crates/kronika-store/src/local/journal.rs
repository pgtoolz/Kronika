//! Reading the raw journal: its header, its frames, and the parts they hold.

use std::io;
use std::path::Path;
use std::sync::Arc;

use kronika_format::{
    Catalog, JOURNAL_HEADER_LEN, JournalHeader, JournalHeaderError, JournalLimits,
    JournalScanError, JournalState, MAX_JOURNAL_LEN, PartRef, RESET_MARKER_LEN, ReadAt,
    ResetMarker, visit_journal_streaming_strict_from,
};
use kronika_layout::{LayoutError, SegmentId};

use crate::source::{ActivePart, JournalScan};

use super::ACTIVE_ARC_ALLOCATION_BYTES;
use super::budget::{layout_io, metadata_limit_io};

#[derive(Debug, Clone, Copy)]
pub(super) struct JournalReadPlan {
    pub(super) segment_id: Option<SegmentId>,
    /// Prefix containing the old frames that must validate.
    pub(super) scan_len: u64,
    /// Complete physical state proven valid by this plan.
    pub(super) valid_len: u64,
    pub(super) committed_reset: bool,
}

pub(super) struct PrefixReader<'a, R> {
    pub(super) inner: &'a R,
    pub(super) len: u64,
}

#[derive(Debug)]
pub(super) struct ActiveJournalScanError(io::Error);

impl std::fmt::Display for ActiveJournalScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "active journal scan: {}", self.0)
    }
}

impl std::error::Error for ActiveJournalScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub(super) fn active_journal_io(error: io::Error) -> io::Error {
    let kind = error.kind();
    io::Error::new(kind, ActiveJournalScanError(error))
}

/// Whether an error returned by [`LocalDir::scan`] originated while reading
/// `active.wal`, rather than while listing or validating finished segments.
#[must_use]
pub fn is_active_journal_scan_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync + 'static>::is::<ActiveJournalScanError>)
}

pub(super) fn active_journal_source(error: &io::Error) -> &io::Error {
    error
        .get_ref()
        .and_then(
            <dyn std::error::Error + Send + Sync + 'static>::downcast_ref::<ActiveJournalScanError>,
        )
        .map_or(error, |wrapped| &wrapped.0)
}

pub(super) fn degradable_active_journal_error(error: &io::Error) -> bool {
    if !is_active_journal_scan_error(error) {
        return false;
    }
    let source = active_journal_source(error);
    if source.kind() == io::ErrorKind::OutOfMemory {
        return false;
    }
    !source.get_ref().is_some_and(|inner| {
        inner.downcast_ref::<LayoutError>().is_some_and(|layout| {
            matches!(
                layout,
                LayoutError::InvalidLimits { .. } | LayoutError::TraversalLimitExceeded { .. }
            )
        })
    })
}

/// Whether an I/O error means the live journal shrank or vanished under us
/// (a concurrent write + `Journal::reset`), rather than a real I/O failure.
pub(super) fn is_stale_journal(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::NotFound
    )
}

pub(super) fn active_part_limit_io(journal_path: &Path, limit: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{} contains more than the allowed {limit} active parts",
            journal_path.display()
        ),
    )
}

pub(super) fn visit_journal_frames<R: ReadAt>(
    reader: &R,
    start_at: u64,
    remaining_parts: usize,
    total_part_limit: usize,
    journal_path: &Path,
    visitor: impl FnMut(PartRef, Catalog, usize) -> io::Result<()>,
) -> io::Result<usize> {
    match visit_journal_streaming_strict_from(
        reader,
        start_at,
        JournalLimits::default(),
        remaining_parts,
        visitor,
    ) {
        Ok(valid_len) => Ok(valid_len),
        Err(JournalScanError::Io(error)) if is_stale_journal(&error) => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("{} changed during scan: {error}", journal_path.display()),
        )),
        Err(JournalScanError::Io(error)) => Err(error),
        Err(JournalScanError::PartLimitExceeded { .. }) => {
            Err(active_part_limit_io(journal_path, total_part_limit))
        }
    }
}

pub(super) fn validate_active_part_reference<R: ReadAt>(
    reader: &R,
    active: &ActivePart,
) -> io::Result<()> {
    let file_len = reader.byte_len()?;
    validate_journal_len(file_len)?;
    if file_len < JOURNAL_HEADER_LEN as u64 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    let mut header_bytes = [0_u8; JOURNAL_HEADER_LEN];
    reader.read_exact_at(&mut header_bytes, 0)?;
    if committed_reset_plan(reader, file_len, header_bytes)?.is_some() {
        return Err(stale_active_generation());
    }
    let header = JournalHeader::decode(header_bytes).map_err(journal_header_io)?;
    let JournalState::Active { segment_id } = header.state else {
        return Err(stale_active_generation());
    };
    if segment_id != active.segment_id.get() {
        return Err(stale_active_generation());
    }

    let committed_end = (JOURNAL_HEADER_LEN as u64)
        .checked_add(header.body_len)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "journal length overflow"))?;
    let part_offset = u64::try_from(active.part.offset)
        .map_err(|_overflow| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    let part_len = u64::try_from(active.part.len)
        .map_err(|_overflow| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    let part_end = part_offset
        .checked_add(part_len)
        .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
    if part_end > committed_end {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    Ok(())
}

pub(super) fn validate_active_snapshot<R: ReadAt>(
    reader: &R,
    expected_segment_id: SegmentId,
    valid_len: u64,
) -> io::Result<()> {
    let file_len = reader.byte_len()?;
    validate_journal_len(file_len)?;
    if file_len < valid_len || file_len < JOURNAL_HEADER_LEN as u64 {
        return Err(stale_active_generation());
    }
    let mut header_bytes = [0_u8; JOURNAL_HEADER_LEN];
    reader.read_exact_at(&mut header_bytes, 0)?;
    if committed_reset_plan(reader, file_len, header_bytes)?.is_some() {
        return Err(stale_active_generation());
    }
    let header = JournalHeader::decode(header_bytes).map_err(journal_header_io)?;
    let JournalState::Active { segment_id } = header.state else {
        return Err(stale_active_generation());
    };
    if segment_id != expected_segment_id.get() {
        return Err(stale_active_generation());
    }
    let committed_end = (JOURNAL_HEADER_LEN as u64)
        .checked_add(header.body_len)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "journal length overflow"))?;
    if committed_end < valid_len {
        return Err(stale_active_generation());
    }
    Ok(())
}

pub(super) fn stale_active_generation() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "active part belongs to another journal generation",
    )
}

pub(super) fn read_journal_plan<R: ReadAt>(reader: &R) -> io::Result<JournalReadPlan> {
    let file_len = reader.byte_len()?;
    validate_journal_len(file_len)?;
    if file_len == 0 {
        // A crash before the writer's first header write; the file provably
        // holds no frames, so it reads as an empty journal.
        return Ok(JournalReadPlan {
            segment_id: None,
            scan_len: 0,
            valid_len: 0,
            committed_reset: false,
        });
    }
    if file_len < JOURNAL_HEADER_LEN as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("active.wal has a torn {file_len}-byte header"),
        ));
    }
    let mut bytes = [0_u8; JOURNAL_HEADER_LEN];
    reader.read_exact_at(&mut bytes, 0)?;
    if let Some(plan) = committed_reset_plan(reader, file_len, bytes)? {
        return Ok(plan);
    }
    let header = JournalHeader::decode(bytes).map_err(journal_header_io)?;
    let actual = file_len - JOURNAL_HEADER_LEN as u64;
    if header.body_len != actual {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "active.wal header records {} body bytes, file contains {actual}",
                header.body_len
            ),
        ));
    }
    match header.state {
        JournalState::Empty if actual == 0 => Ok(JournalReadPlan {
            segment_id: None,
            scan_len: file_len,
            valid_len: file_len,
            committed_reset: false,
        }),
        JournalState::Empty => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty active.wal has trailing frames",
        )),
        JournalState::Active { segment_id } => {
            if actual == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "active active.wal has no complete first frame",
                ));
            }
            Ok(JournalReadPlan {
                segment_id: Some(SegmentId::new(segment_id).map_err(layout_io)?),
                scan_len: file_len,
                valid_len: file_len,
                committed_reset: false,
            })
        }
    }
}

pub(super) fn validate_journal_len(file_len: u64) -> io::Result<()> {
    if file_len > MAX_JOURNAL_LEN as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "active.wal length {file_len} exceeds the version-1 limit of {MAX_JOURNAL_LEN} bytes"
            ),
        ));
    }
    Ok(())
}

pub(super) fn committed_reset_plan<R: ReadAt>(
    reader: &R,
    file_len: u64,
    header_bytes: [u8; JOURNAL_HEADER_LEN],
) -> io::Result<Option<JournalReadPlan>> {
    let marker_len = RESET_MARKER_LEN as u64;
    let Some(marker_at) = file_len.checked_sub(marker_len) else {
        return Ok(None);
    };
    if marker_at <= JOURNAL_HEADER_LEN as u64 {
        return Ok(None);
    }
    let mut marker_bytes = [0_u8; RESET_MARKER_LEN];
    reader.read_exact_at(&mut marker_bytes, marker_at)?;
    let Some(marker) = ResetMarker::decode(marker_bytes) else {
        return Ok(None);
    };
    if marker.previous_len != marker_at
        || marker.expected_previous_header().is_none()
        || marker.classify_header_transition(header_bytes).is_none()
    {
        return Ok(None);
    }
    let Ok(segment_id) = SegmentId::new(marker.previous_segment_id) else {
        return Ok(None);
    };
    Ok(Some(JournalReadPlan {
        segment_id: Some(segment_id),
        scan_len: marker.previous_len,
        valid_len: file_len,
        committed_reset: true,
    }))
}

pub(super) fn journal_header_io(error: JournalHeaderError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub(super) fn empty_journal_scan(metadata_limit: usize) -> io::Result<JournalScan> {
    if ACTIVE_ARC_ALLOCATION_BYTES > metadata_limit {
        return Err(metadata_limit_io(metadata_limit));
    }
    Ok(JournalScan {
        active: Arc::new(Vec::new()),
        valid_len: 0,
        committed_reset: false,
        metadata_bytes: ACTIVE_ARC_ALLOCATION_BYTES,
    })
}

impl<R: ReadAt> ReadAt for PrefixReader<'_, R> {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let buf_len = u64::try_from(buf.len())
            .map_err(|_overflow| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let end = offset
            .checked_add(buf_len)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        if end > self.len {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        self.inner.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        Ok(self.len)
    }
}
