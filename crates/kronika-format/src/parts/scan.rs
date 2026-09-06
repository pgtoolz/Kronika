//! Walking a journal's frames through a reader.

use super::{
    Catalog, Error, FRAME_HEADER_LEN, FrameHeader, MAX_PART_LEN, ReadAt, fmt, io, validate_part,
};

/// Limits used while scanning a journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalLimits {
    /// Frames claiming a part longer than this are rejected.
    pub max_part_len: u64,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            max_part_len: MAX_PART_LEN,
        }
    }
}

/// Location of one valid part body inside the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartRef {
    /// Offset of the part body (after the frame header).
    pub offset: usize,
    /// Length of the part body, bytes.
    pub len: usize,
}

/// Result of scanning a journal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanReport {
    /// Valid parts in journal order.
    pub parts: Vec<PartRef>,
    /// Length of the journal prefix ending at the last valid frame. A scan
    /// that stopped before the end of the source found damage there.
    pub valid_len: usize,
}

/// Failure while scanning a journal with an explicit part-count bound.
#[derive(Debug)]
pub enum JournalScanError {
    /// The byte source could not be read.
    Io(io::Error),
    /// Another valid frame would exceed the caller's part-count bound.
    PartLimitExceeded {
        /// Maximum number of returned parts.
        limit: usize,
    },
}

impl fmt::Display for JournalScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "journal scan I/O: {error}"),
            Self::PartLimitExceeded { limit } => {
                write!(f, "journal contains more than {limit} parts")
            }
        }
    }
}

impl Error for JournalScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::PartLimitExceeded { .. } => None,
        }
    }
}

impl From<io::Error> for JournalScanError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Scans a journal source sequentially from `start_at`.
///
/// `start_at` must be a frame boundary. Returned part offsets, and
/// [`ScanReport::valid_len`], remain absolute from the start of the source. If
/// no bytes follow `start_at`, the report is empty and `valid_len` equals
/// `start_at`.
///
/// Scanning stops at the first damaged frame and never searches the damaged
/// bytes for candidate frame magic. Peak memory is one part body and at most
/// `max_parts` references. The part limit is checked before adding each
/// [`PartRef`] to the report.
///
/// # Errors
///
/// Returns [`JournalScanError::PartLimitExceeded`] before returning more than
/// `max_parts` valid frames. A `start_at` beyond the source and failures from
/// the source are returned as [`JournalScanError::Io`].
pub fn scan_journal_streaming_strict_from<R: ReadAt>(
    reader: &R,
    start_at: u64,
    limits: JournalLimits,
    max_parts: usize,
) -> Result<ScanReport, JournalScanError> {
    let mut parts = Vec::new();
    let valid_len = visit_journal_streaming_strict_from(
        reader,
        start_at,
        limits,
        max_parts,
        |part, _catalog, _part_buffer_capacity| {
            parts.push(part);
            Ok(())
        },
    )?;
    Ok(ScanReport { parts, valid_len })
}

/// Visits a journal and hands each validated catalog to `visitor` before the
/// reusable part buffer is overwritten.
///
/// Unlike [`scan_journal_streaming_strict_from`], this function does not retain
/// a vector of part references. Its result is the length of the valid prefix.
/// The final visitor argument is the retained capacity of the reusable part
/// buffer, for callers that enforce a memory budget.
///
/// # Errors
///
/// Returns the same failures as [`scan_journal_streaming_strict_from`], plus
/// I/O errors returned by `visitor`.
pub fn visit_journal_streaming_strict_from<R: ReadAt>(
    reader: &R,
    start_at: u64,
    limits: JournalLimits,
    max_parts: usize,
    mut visitor: impl FnMut(PartRef, Catalog, usize) -> io::Result<()>,
) -> Result<usize, JournalScanError> {
    let total_len = usize::try_from(reader.byte_len()?).map_err(|_overflow| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "source does not fit the address space",
        )
    })?;
    let mut pos = usize::try_from(start_at).map_err(|_overflow| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "start_at does not fit the address space",
        )
    })?;
    if pos > total_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "start_at is beyond the journal source",
        )
        .into());
    }

    let mut part_count = 0_usize;
    let mut part_buf = Vec::new();

    while pos < total_len {
        match streaming_frame_at(reader, total_len, pos, limits, &mut part_buf)? {
            StreamingFrame::Valid { body_len, catalog } => {
                if part_count >= max_parts {
                    return Err(JournalScanError::PartLimitExceeded { limit: max_parts });
                }
                let part = PartRef {
                    offset: pos + FRAME_HEADER_LEN,
                    len: body_len,
                };
                part_count += 1;
                pos += FRAME_HEADER_LEN + body_len;
                visitor(part, catalog, part_buf.capacity())?;
            }
            StreamingFrame::Damaged => return Ok(pos),
        }
    }

    Ok(pos)
}

/// One frame position as the streaming scanner found it.
enum StreamingFrame {
    /// A complete, fully validated frame body of this length.
    Valid { body_len: usize, catalog: Catalog },
    /// The position holds no usable frame, whatever the reason.
    Damaged,
}

fn streaming_frame_at<R: ReadAt>(
    reader: &R,
    total_len: usize,
    pos: usize,
    limits: JournalLimits,
    part_buf: &mut Vec<u8>,
) -> io::Result<StreamingFrame> {
    let rem = total_len - pos;
    if rem < FRAME_HEADER_LEN {
        return Ok(StreamingFrame::Damaged);
    }
    let mut header_bytes = [0_u8; FRAME_HEADER_LEN];
    reader.read_exact_at(&mut header_bytes, pos as u64)?;
    let Ok(header) = FrameHeader::decode(header_bytes) else {
        return Ok(StreamingFrame::Damaged);
    };
    if header.part_len > limits.max_part_len {
        return Ok(StreamingFrame::Damaged);
    }
    let Ok(body_len) = usize::try_from(header.part_len) else {
        return Ok(StreamingFrame::Damaged);
    };
    if rem - FRAME_HEADER_LEN < body_len {
        return Ok(StreamingFrame::Damaged);
    }
    part_buf.resize(body_len, 0);
    reader.read_exact_at(&mut part_buf[..body_len], (pos + FRAME_HEADER_LEN) as u64)?;
    let Ok(catalog) = validate_part(&part_buf[..body_len]) else {
        return Ok(StreamingFrame::Damaged);
    };
    if catalog
        .entries
        .iter()
        .try_fold(0_u64, |rows, entry| rows.checked_add(u64::from(entry.rows)))
        .is_none()
    {
        return Ok(StreamingFrame::Damaged);
    }
    Ok(StreamingFrame::Valid { body_len, catalog })
}
