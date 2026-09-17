//! Storage-form-neutral row selection and finished-ZMS construction.

mod selection;
mod staging;

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Seek as _, SeekFrom, Write};

use kronika_layout::SegmentId;
use kronika_reader::{Listing, Reader, ReaderError, StoreObject, StoreWarning, StoreWarningReason};
use kronika_registry::{CodecError, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID};
use kronika_store::ActiveJournalWarningReason;
use kronika_writer::{FinishedZmsPlan, WriteError, write_finished_zms};

use self::selection::select_boundaries;
use self::staging::stage_selected_rows;

const MICROS_PER_SECOND: i64 = 1_000_000;
// Counter/rate calculations need neighboring observations, bounded to 30 seconds.
const CONTEXT_MICROS: i64 = 30 * MICROS_PER_SECOND;
// The storage layout accepts years 0000..=9999; context is clipped at those edges.
const LAYOUT_MIN_MICROS: i64 = -62_167_219_200_000_000;
const LAYOUT_MAX_EXCLUSIVE_MICROS: i64 = 253_402_300_800_000_000;

/// One whole UTC second accepted by the storage layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcSecond(i64);

impl UtcSecond {
    /// Validate a Unix-second value for use as a slice endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError`] when converting to microseconds overflows or the
    /// resulting time lies outside the storage layout.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, RangeError> {
        let micros = seconds
            .checked_mul(MICROS_PER_SECOND)
            .ok_or(RangeError::OutOfRange)?;
        SegmentId::new(micros).map_err(|_problem| RangeError::OutOfRange)?;
        Ok(Self(seconds))
    }

    /// Unix seconds since 1970-01-01T00:00:00Z.
    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.0
    }

    const fn unix_micros(self) -> i64 {
        self.0 * MICROS_PER_SECOND
    }
}

/// Invalid slice endpoint or endpoint order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RangeError {
    /// An endpoint is outside years 0000 through 9999.
    OutOfRange,
    /// The first endpoint is later than the last endpoint.
    Reversed,
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange => f.write_str("timestamp is outside years 0000 through 9999"),
            Self::Reversed => f.write_str("the first second is later than the last second"),
        }
    }
}

impl Error for RangeError {}

/// Inclusive whole-second slice bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceRange {
    from: UtcSecond,
    to: UtcSecond,
}

impl SliceRange {
    /// Construct inclusive whole-second bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RangeError::Reversed`] when `from` is later than `to`.
    pub const fn new(from: UtcSecond, to: UtcSecond) -> Result<Self, RangeError> {
        if from.0 > to.0 {
            Err(RangeError::Reversed)
        } else {
            Ok(Self { from, to })
        }
    }

    /// Inclusive first second.
    #[must_use]
    pub const fn from(self) -> UtcSecond {
        self.from
    }

    /// Inclusive last second.
    #[must_use]
    pub const fn to(self) -> UtcSecond {
        self.to
    }

    fn micros(self) -> MicrosRange {
        let from = self.from.unix_micros();
        // UtcSecond already bounds endpoints to the layout; the exclusive end
        // of its final valid second is still safely inside i64.
        let to_exclusive = self.to.unix_micros() + MICROS_PER_SECOND;
        MicrosRange {
            from,
            to_exclusive,
            context_from: from.saturating_sub(CONTEXT_MICROS).max(LAYOUT_MIN_MICROS),
            context_to_exclusive: to_exclusive
                .saturating_add(CONTEXT_MICROS)
                .min(LAYOUT_MAX_EXCLUSIVE_MICROS),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MicrosRange {
    from: i64,
    to_exclusive: i64,
    context_from: i64,
    context_to_exclusive: i64,
}

impl MicrosRange {
    const fn in_request(self, ts: i64) -> bool {
        self.from <= ts && ts < self.to_exclusive
    }

    const fn before(self, ts: i64) -> bool {
        self.context_from <= ts && ts < self.from
    }

    const fn after(self, ts: i64) -> bool {
        self.to_exclusive <= ts && ts < self.context_to_exclusive
    }
}

/// Facts about one completed standalone slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceSummary {
    /// Segment identity derived from the requested first second.
    pub segment_id: SegmentId,
    /// Requested first instant, unix microseconds.
    pub requested_from: i64,
    /// Exclusive end immediately after the requested last second.
    pub requested_to_exclusive: i64,
    /// Earliest timestamp actually encoded.
    pub actual_min_ts: i64,
    /// Latest timestamp actually encoded.
    pub actual_max_ts: i64,
    /// Number of non-dictionary rows encoded.
    pub rows_written: u64,
    /// Number of physical data and dictionary sections.
    pub sections_written: usize,
    /// Complete finished ZMS byte length.
    pub bytes_written: u64,
}

/// Failure to select or mechanically encode one standalone slice.
#[derive(Debug)]
#[non_exhaustive]
pub enum SliceError {
    /// The requested range was invalid.
    Range(RangeError),
    /// Production storage/section reading failed.
    Reader(ReaderError),
    /// A registry codec rejected source or selected rows.
    Codec(CodecError),
    /// Final finished-segment construction failed.
    Writer(WriteError),
    /// Scratch or output I/O failed.
    Io(io::Error),
    /// No stored row fell within the requested seconds.
    NoRowsInRequestedRange,
    /// A physical non-dictionary type has no usable time axis.
    UnsliceableType {
        /// Physical type id that cannot be selected by time.
        type_id: u32,
    },
    /// A selected dictionary id was absent from its source segment.
    UnresolvedDictionary {
        /// Missing raw dictionary id.
        str_id: u64,
    },
    /// A source selected for the bounded range was unreadable.
    RequiredRangeUnreadable(Box<StoreWarning>),
    /// A selected Arrow batch did not match its registered contract.
    InvalidBatch {
        /// Physical type id being selected.
        type_id: u32,
        /// Column that could not be read.
        column: &'static str,
    },
    /// A bounded row or byte count overflowed.
    ArithmeticOverflow,
}

impl fmt::Display for SliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Range(problem) => problem.fmt(f),
            Self::Reader(problem) => problem.fmt(f),
            Self::Codec(problem) => problem.fmt(f),
            Self::Writer(problem) => problem.fmt(f),
            Self::Io(problem) => write!(f, "slice io: {problem}"),
            Self::NoRowsInRequestedRange => {
                f.write_str("no rows were recorded in the requested range")
            }
            Self::UnsliceableType { type_id } => {
                write!(f, "section {type_id} has no usable timestamp column")
            }
            Self::UnresolvedDictionary { str_id } => {
                write!(f, "dictionary id {str_id} is unresolved")
            }
            Self::RequiredRangeUnreadable(warning) => write!(
                f,
                "required storage object {:?} is unreadable: reason={} identity={:?} failure={:?}",
                warning.affected,
                warning.reason.code(),
                warning.identity,
                warning.failure
            ),
            Self::InvalidBatch { type_id, column } => {
                write!(f, "section {type_id} has an invalid {column} column")
            }
            Self::ArithmeticOverflow => f.write_str("slice row or byte count overflow"),
        }
    }
}

impl Error for SliceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Range(problem) => Some(problem),
            Self::Reader(problem) => Some(problem),
            Self::Codec(problem) => Some(problem),
            Self::Writer(problem) => Some(problem),
            Self::Io(problem) => Some(problem),
            Self::NoRowsInRequestedRange
            | Self::UnsliceableType { .. }
            | Self::UnresolvedDictionary { .. }
            | Self::RequiredRangeUnreadable(_)
            | Self::InvalidBatch { .. }
            | Self::ArithmeticOverflow => None,
        }
    }
}

impl From<RangeError> for SliceError {
    fn from(problem: RangeError) -> Self {
        Self::Range(problem)
    }
}

impl From<ReaderError> for SliceError {
    fn from(problem: ReaderError) -> Self {
        Self::Reader(problem)
    }
}

impl From<CodecError> for SliceError {
    fn from(problem: CodecError) -> Self {
        Self::Codec(problem)
    }
}

impl From<WriteError> for SliceError {
    fn from(problem: WriteError) -> Self {
        Self::Writer(problem)
    }
}

impl From<io::Error> for SliceError {
    fn from(problem: io::Error) -> Self {
        Self::Io(problem)
    }
}

/// Select `range` and write one finished standalone ZMS.
///
/// `scratch` is caller-owned disk space. Its previous contents are discarded,
/// and it is reused if one source-change retry is required. The output is not
/// touched until source selection, decoding, and final section construction
/// have completed successfully.
///
/// # Errors
///
/// Returns [`SliceError`] for unreadable selected storage, an empty requested
/// interval, unresolved dictionary values, or an encoding/output failure.
pub fn slice_to_zms(
    reader: &Reader,
    range: SliceRange,
    scratch: &mut File,
    output: &mut impl Write,
) -> Result<SliceSummary, SliceError> {
    let range = range.micros();
    // Rotation can invalidate the captured active journal between the two passes.
    // Retry the entire capture once; never append partially prepared output.
    let prepared = match prepare_slice(reader, range, scratch) {
        Ok(prepared) => prepared,
        Err(problem) if retryable_source_change(&problem) => prepare_slice(reader, range, scratch)?,
        Err(problem) => return Err(problem),
    };
    let written = write_finished_zms(scratch, &prepared.plan, output)?;
    Ok(SliceSummary {
        segment_id: prepared.segment_id,
        requested_from: range.from,
        requested_to_exclusive: range.to_exclusive,
        actual_min_ts: prepared.actual_min_ts,
        actual_max_ts: prepared.actual_max_ts,
        rows_written: prepared.rows_written,
        sections_written: written.sections,
        bytes_written: written.bytes,
    })
}

struct PreparedSlice {
    plan: FinishedZmsPlan,
    segment_id: SegmentId,
    actual_min_ts: i64,
    actual_max_ts: i64,
    rows_written: u64,
}

fn prepare_slice(
    reader: &Reader,
    range: MicrosRange,
    scratch: &mut File,
) -> Result<PreparedSlice, SliceError> {
    scratch.set_len(0)?;
    scratch.seek(SeekFrom::Start(0))?;
    let listing = reader.segments(range.context_from..range.context_to_exclusive)?;
    prepare_captured_slice(reader, range, &listing, scratch)
}

fn prepare_captured_slice(
    reader: &Reader,
    range: MicrosRange,
    listing: &Listing,
    scratch: &mut File,
) -> Result<PreparedSlice, SliceError> {
    check_warnings(listing)?;

    let selections = select_boundaries(reader, &listing.segments, range)?;
    if !selections
        .values()
        .any(|selection| selection.has_requested_rows)
    {
        return Err(SliceError::NoRowsInRequestedRange);
    }

    #[cfg(test)]
    tests::AFTER_SELECTION_PASS.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });

    let segment_id =
        SegmentId::new(range.from).map_err(|_problem| SliceError::Range(RangeError::OutOfRange))?;
    let selected = stage_selected_rows(reader, &listing.segments, &selections, range, scratch)?;
    if selected.rows_written == 0 || selected.actual_min_ts > selected.actual_max_ts {
        return Err(SliceError::NoRowsInRequestedRange);
    }
    let sections = selected.finalize(scratch)?;
    let plan = FinishedZmsPlan::new(sections, selected.actual_min_ts, selected.actual_max_ts, 0)?;
    Ok(PreparedSlice {
        plan,
        segment_id,
        actual_min_ts: selected.actual_min_ts,
        actual_max_ts: selected.actual_max_ts,
        rows_written: selected.rows_written,
    })
}

fn retryable_source_change(problem: &SliceError) -> bool {
    match problem {
        SliceError::Reader(problem) => problem.source_changed_during_read(),
        SliceError::RequiredRangeUnreadable(warning) => matches!(
            warning.reason,
            StoreWarningReason::ActiveJournal(ActiveJournalWarningReason::Io)
        ),
        _ => false,
    }
}

fn check_warnings(listing: &Listing) -> Result<(), SliceError> {
    if let Some(warning) = listing.warnings.iter().find(|warning| {
        matches!(
            (warning.affected, warning.reason),
            (StoreObject::Segment(_), StoreWarningReason::InvalidZms(_))
                | (
                    StoreObject::ActiveJournal,
                    StoreWarningReason::ActiveJournal(_)
                )
        )
    }) {
        return Err(SliceError::RequiredRangeUnreadable(Box::new(*warning)));
    }
    Ok(())
}

const fn is_dictionary(type_id: u32) -> bool {
    matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID)
}

#[cfg(test)]
#[path = "tests/slice.rs"]
mod tests;
