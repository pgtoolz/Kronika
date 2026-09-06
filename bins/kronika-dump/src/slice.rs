//! Storage-form-neutral row selection and finished-ZMS construction.

use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, BufWriter, Seek as _, SeekFrom, Write};
use std::os::unix::fs::FileExt as _;

use arrow_array::{Array as _, BooleanArray, Int64Array, RecordBatch, UInt64Array};
use arrow_select::filter::filter_record_batch;
use kronika_format::{Crc32c, StrId};
use kronika_layout::SegmentId;
use kronika_reader::{
    Listing, OwnedDictionaryValue, Reader, ReaderError, SegmentRef, StoreObject, StoreWarning,
    StoreWarningReason,
};
use kronika_registry::{
    Bytes, CodecError, ColumnType, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, Semantics,
    TypeContract, contract, encode_final_batches, encode_final_sections_to,
};
use kronika_store::ActiveJournalWarningReason;
use kronika_writer::{
    FinishedDictionary, FinishedSection, FinishedZmsPlan, WriteError, write_finished_zms,
};

const MICROS_PER_SECOND: i64 = 1_000_000;
const CONTEXT_MICROS: i64 = 30 * MICROS_PER_SECOND;
const LAYOUT_MIN_MICROS: i64 = -62_167_219_200_000_000;
const LAYOUT_MAX_EXCLUSIVE_MICROS: i64 = 253_402_300_800_000_000;

#[cfg(test)]
std::thread_local! {
    static AFTER_SELECTION_PASS: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        std::cell::RefCell::new(None);
}

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

    fn micros(self) -> Result<MicrosRange, RangeError> {
        let from = self.from.unix_micros();
        let to_exclusive = self
            .to
            .unix_micros()
            .checked_add(MICROS_PER_SECOND)
            .ok_or(RangeError::OutOfRange)?;
        Ok(MicrosRange {
            from,
            to_exclusive,
            context_from: from.saturating_sub(CONTEXT_MICROS).max(LAYOUT_MIN_MICROS),
            context_to_exclusive: to_exclusive
                .saturating_add(CONTEXT_MICROS)
                .min(LAYOUT_MAX_EXCLUSIVE_MICROS),
        })
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
    let range = range.micros()?;
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

    let mut selections = selection_pass(reader, &listing.segments, range)?;
    if !selections.values().any(|selection| selection.in_request) {
        return Err(SliceError::NoRowsInRequestedRange);
    }

    #[cfg(test)]
    AFTER_SELECTION_PASS.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });

    let segment_id =
        SegmentId::new(range.from).map_err(|_problem| SliceError::Range(RangeError::OutOfRange))?;
    let selected = stage_selected_rows(reader, &listing.segments, &mut selections, range, scratch)?;
    if selected.rows_written == 0 || selected.actual_min_ts > selected.actual_max_ts {
        return Err(SliceError::NoRowsInRequestedRange);
    }
    let staging = scratch.try_clone()?;
    let sections = {
        let mut spool_writer = BufWriter::new(&mut *scratch);
        let mut sections = finalize_data(
            &staging,
            &selected.staged,
            &mut spool_writer,
            selected.staging_end,
        )?;
        let offset = sections
            .iter()
            .try_fold(selected.staging_end, |end, section| {
                section
                    .offset()
                    .checked_add(section.len())
                    .map(|next| end.max(next))
            })
            .ok_or(SliceError::ArithmeticOverflow)?;
        sections.extend(
            selected
                .dictionary
                .write_sections_to(&mut spool_writer, offset)?,
        );
        spool_writer.flush()?;
        sections
    };
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

#[derive(Debug)]
struct StagedSelection {
    staged: BTreeMap<u32, Vec<StagedSection>>,
    dictionary: FinishedDictionary,
    staging_end: u64,
    actual_min_ts: i64,
    actual_max_ts: i64,
    rows_written: u64,
}

fn stage_selected_rows(
    reader: &Reader,
    references: &[SegmentRef],
    selections: &mut BTreeMap<u32, TypeSelection>,
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
    let mut staging_offset = 0_u64;
    for reference in references {
        let segment = reader.open_segment(reference)?;
        let mut dictionary_ids = HashSet::new();
        for type_id in segment.type_ids() {
            if is_dictionary(type_id) {
                continue;
            }
            let selection = selections
                .get_mut(&type_id)
                .ok_or(SliceError::UnsliceableType { type_id })?;
            let contract = selection.contract;
            let mut callback_error = None;
            segment.visit_batches(type_id, None, 0, usize::MAX, |_ordinal, batch| {
                let Some((retained, min_ts, max_ts)) = (match retain_batch(&batch, selection, range)
                {
                    Ok(retained) => retained,
                    Err(problem) => {
                        callback_error = Some(problem);
                        return false;
                    }
                }) else {
                    return true;
                };
                match stage_batch(
                    staging,
                    &mut staging_offset,
                    type_id,
                    retained,
                    &mut selected.staged,
                    &mut dictionary_ids,
                    contract,
                ) {
                    Ok(rows) => {
                        let Some(total) = selected.rows_written.checked_add(rows) else {
                            callback_error = Some(SliceError::ArithmeticOverflow);
                            return false;
                        };
                        selected.rows_written = total;
                        selected.actual_min_ts = selected.actual_min_ts.min(min_ts);
                        selected.actual_max_ts = selected.actual_max_ts.max(max_ts);
                        true
                    }
                    Err(problem) => {
                        callback_error = Some(problem);
                        false
                    }
                }
            })?;
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
    selected.staging_end = staging_offset;
    Ok(selected)
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

#[derive(Debug)]
struct TypeSelection {
    contract: &'static TypeContract,
    in_request: bool,
    boundary: Boundary,
}

#[derive(Debug)]
enum Boundary {
    None,
    AllContext,
    Cohort {
        before: Option<i64>,
        after: Option<i64>,
    },
}

fn selection_pass(
    reader: &Reader,
    references: &[SegmentRef],
    range: MicrosRange,
) -> Result<BTreeMap<u32, TypeSelection>, SliceError> {
    let mut selections = BTreeMap::new();
    for reference in references {
        let segment = reader.open_segment(reference)?;
        for type_id in segment.type_ids() {
            if is_dictionary(type_id) {
                continue;
            }
            let contract = contract(type_id).ok_or(SliceError::UnsliceableType { type_id })?;
            let timestamp =
                timestamp_column(contract).ok_or(SliceError::UnsliceableType { type_id })?;
            let selection = selections.entry(type_id).or_insert_with(|| TypeSelection {
                contract,
                in_request: false,
                boundary: match contract.semantics {
                    Semantics::EventStream => Boundary::None,
                    Semantics::Changed | Semantics::OnChange => Boundary::AllContext,
                    Semantics::SnapshotFull | Semantics::ConditionalFull => Boundary::Cohort {
                        before: None,
                        after: None,
                    },
                },
            });
            let mut callback_error = None;
            segment.visit_batches(
                type_id,
                Some(&[timestamp]),
                0,
                usize::MAX,
                |_ordinal, batch| {
                    let Some(values) = batch
                        .column_by_name(timestamp)
                        .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
                    else {
                        callback_error = Some(SliceError::InvalidBatch {
                            type_id,
                            column: timestamp,
                        });
                        return false;
                    };
                    if values.null_count() != 0 {
                        callback_error = Some(SliceError::InvalidBatch {
                            type_id,
                            column: timestamp,
                        });
                        return false;
                    }
                    for value in values.values() {
                        observe_timestamp(selection, *value, range);
                    }
                    true
                },
            )?;
            if let Some(problem) = callback_error {
                return Err(problem);
            }
        }
    }
    Ok(selections)
}

fn timestamp_column(contract: &TypeContract) -> Option<&'static str> {
    let mut timestamps = contract
        .columns
        .iter()
        .filter(|column| column.class == kronika_registry::ColumnClass::Timestamp);
    let column = timestamps.next()?;
    (timestamps.next().is_none() && column.ty == ColumnType::Ts && !column.nullable)
        .then_some(column.name)
}

fn timestamps<'a>(
    batch: &'a RecordBatch,
    contract: &TypeContract,
) -> Result<&'a Int64Array, SliceError> {
    let name = timestamp_column(contract).ok_or(SliceError::UnsliceableType {
        type_id: contract.type_id.get(),
    })?;
    batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<Int64Array>())
        .ok_or(SliceError::InvalidBatch {
            type_id: contract.type_id.get(),
            column: name,
        })
}

fn observe_timestamp(selection: &mut TypeSelection, value: i64, range: MicrosRange) {
    if range.in_request(value) {
        selection.in_request = true;
        return;
    }
    match &mut selection.boundary {
        Boundary::Cohort { before, after } if range.before(value) => {
            *before = Some(before.map_or(value, |current| current.max(value)));
        }
        Boundary::Cohort { after, .. } if range.after(value) => {
            *after = Some(after.map_or(value, |current| current.min(value)));
        }
        Boundary::None | Boundary::AllContext | Boundary::Cohort { .. } => {}
    }
}

fn retain_batch(
    batch: &RecordBatch,
    selection: &TypeSelection,
    range: MicrosRange,
) -> Result<Option<(RecordBatch, i64, i64)>, SliceError> {
    let ts = timestamps(batch, selection.contract)?;
    let mut mask = Vec::with_capacity(batch.num_rows());
    let mut min_ts = i64::MAX;
    let mut max_ts = i64::MIN;
    for row in 0..batch.num_rows() {
        if ts.is_null(row) {
            return Err(SliceError::InvalidBatch {
                type_id: selection.contract.type_id.get(),
                column: "ts",
            });
        }
        let value = ts.value(row);
        let retain = if range.in_request(value) {
            true
        } else {
            match &selection.boundary {
                Boundary::None => false,
                Boundary::AllContext => range.before(value) || range.after(value),
                Boundary::Cohort { before, after } => {
                    *before == Some(value) || *after == Some(value)
                }
            }
        };
        mask.push(retain);
        if retain {
            min_ts = min_ts.min(value);
            max_ts = max_ts.max(value);
        }
    }
    if min_ts > max_ts {
        return Ok(None);
    }
    let retained =
        filter_record_batch(batch, &BooleanArray::from(mask)).map_err(CodecError::from)?;
    Ok(Some((retained, min_ts, max_ts)))
}

#[derive(Debug, Clone, Copy)]
struct StagedSection {
    offset: u64,
    len: u64,
    rows: u32,
}

fn stage_batch(
    staging: &mut File,
    staging_offset: &mut u64,
    type_id: u32,
    batch: RecordBatch,
    staged: &mut BTreeMap<u32, Vec<StagedSection>>,
    dictionary_ids: &mut HashSet<u64>,
    contract: &TypeContract,
) -> Result<u64, SliceError> {
    collect_dictionary_ids(&batch, contract, dictionary_ids)?;
    let rows =
        u32::try_from(batch.num_rows()).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
    let body = encode_final_batches(type_id, vec![batch])?;
    let len = u64::try_from(body.len()).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
    let offset = *staging_offset;
    let next = offset
        .checked_add(len)
        .ok_or(SliceError::ArithmeticOverflow)?;
    staging.write_all(&body)?;
    *staging_offset = next;
    staged
        .entry(type_id)
        .or_default()
        .push(StagedSection { offset, len, rows });
    Ok(u64::from(rows))
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

fn finalize_data(
    staging: &File,
    staged: &BTreeMap<u32, Vec<StagedSection>>,
    spool: &mut (impl Write + Send),
    mut offset: u64,
) -> Result<Vec<FinishedSection>, SliceError> {
    let mut sections = Vec::with_capacity(staged.len());
    for (&type_id, sources) in staged {
        let rows = sources.iter().map(|source| source.rows).collect::<Vec<_>>();
        let total_rows = rows.iter().try_fold(0_u32, |total, rows| {
            total
                .checked_add(*rows)
                .ok_or(SliceError::ArithmeticOverflow)
        })?;
        let mut sink = SectionSink::new(&mut *spool);
        encode_final_sections_to(type_id, &rows, &mut sink, |index| {
            let source = sources.get(index).ok_or(SliceError::ArithmeticOverflow)?;
            staged_body(staging, *source)
        })?;
        let (len, checksum) = sink.finish();
        sections.push(FinishedSection::new(
            type_id, total_rows, offset, len, checksum,
        )?);
        offset = offset
            .checked_add(len)
            .ok_or(SliceError::ArithmeticOverflow)?;
    }
    Ok(sections)
}

fn staged_body(staging: &File, staged: StagedSection) -> Result<Bytes, SliceError> {
    let len = usize::try_from(staged.len).map_err(|_overflow| SliceError::ArithmeticOverflow)?;
    let mut body = vec![0_u8; len];
    staging.read_exact_at(&mut body, staged.offset)?;
    Ok(Bytes::from(body))
}

const fn is_dictionary(type_id: u32) -> bool {
    matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID)
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

#[cfg(test)]
mod tests;
