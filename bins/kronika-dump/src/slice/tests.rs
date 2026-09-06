use std::cell::Cell as Counter;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kronika_format::{DictLimits, Resolved};
use kronika_index::{SeriesBlock, build};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress};
use kronika_reader::{Cell, FinishedReader};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::os_user::OsUser;
use kronika_registry::{StrId as RegistryStrId, Ts};
use kronika_report::{HtmlReportInput, write_html};
use kronika_store::EmbeddedSource;
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};

use super::*;

const BASE_SECOND: i64 = 1_700_000_000;
const BASE: i64 = BASE_SECOND * MICROS_PER_SECOND;
const LOAD_TYPE: u32 = 1_105_001;
const USER_TYPE: u32 = 1_124_002;
const METADATA_TYPE: u32 = 1_021_002;

fn load(ts: i64, value: f64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(ts),
        load1: value,
        load5: value,
        load15: value,
        running: 1,
        total: 2,
        scope: 0,
    }
}

fn encoded_load_part(rows: &[(i64, f64)]) -> Vec<u8> {
    let mut buffers = SectionBuffers::new();
    for &(ts, value) in rows {
        buffers.push(load(ts, value)).expect("buffer load row");
    }
    buffers
        .flush(&[])
        .expect("encode load part")
        .expect("nonempty load part")
}

fn append_load_segment(root_path: &Path, rows: &[(i64, f64)], finished: bool) {
    let root = DataRoot::open(root_path).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let id = SegmentId::new(rows[0].0).expect("fixture segment id");
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open fixture journal");
    let part = encoded_load_part(rows);
    journal.append(id, &part).expect("append fixture part");
    if finished {
        write_segment(
            &journal,
            &owner,
            SegmentAddress::new(id).expect("fixture address"),
        )
        .expect("finish fixture segment");
        journal.reset().expect("reset fixture journal");
    }
}

fn output_segment(bytes: Vec<u8>, segment_id: SegmentId) -> kronika_reader::Segment {
    let max = bytes.len() as u64;
    let source = EmbeddedSource::from_owned(segment_id, bytes, max).expect("valid output ZMS");
    let reader = FinishedReader::new(source);
    let listing = reader.resources().expect("list embedded output");
    assert_eq!(listing.resources.len(), 1);
    reader
        .open_segment(&listing.resources[0])
        .expect("open embedded output")
}

fn timestamps_of(segment: &kronika_reader::Segment, type_id: u32) -> Vec<i64> {
    segment
        .rows(type_id)
        .expect("decode selected rows")
        .iter()
        .map(|row| match row.get("ts") {
            Some(Cell::Ts(ts)) => *ts,
            _ => i64::MIN,
        })
        .collect()
}

fn whole_second_range(first: i64, last: i64) -> SliceRange {
    SliceRange::new(
        UtcSecond::from_unix_seconds(first).expect("first second"),
        UtcSecond::from_unix_seconds(last).expect("last second"),
    )
    .expect("ordered fixture range")
}

fn slice_to_zms(
    reader: &Reader,
    range: SliceRange,
    output: &mut impl Write,
) -> Result<SliceSummary, SliceError> {
    let mut scratch = tempfile::tempfile().map_err(SliceError::from)?;
    super::slice_to_zms(reader, range, &mut scratch, output)
}

fn slice_captured_to_zms(
    reader: &Reader,
    range: MicrosRange,
    listing: &Listing,
    output: &mut impl Write,
) -> Result<SliceSummary, SliceError> {
    let mut scratch = tempfile::tempfile().map_err(SliceError::from)?;
    let prepared = prepare_captured_slice(reader, range, listing, &mut scratch)?;
    let written = write_finished_zms(&scratch, &prepared.plan, output)?;
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

struct SelectionHookGuard;

impl Drop for SelectionHookGuard {
    fn drop(&mut self) {
        AFTER_SELECTION_PASS.with(|slot| {
            slot.replace(None);
        });
    }
}

fn after_selection(hook: impl FnMut() + 'static) -> SelectionHookGuard {
    AFTER_SELECTION_PASS.with(|slot| {
        assert!(slot.replace(Some(Box::new(hook))).is_none());
    });
    SelectionHookGuard
}

fn finish_and_start_next(
    journal: &mut Journal,
    owner: &kronika_layout::WriterOwner,
    current_ts: &mut i64,
) {
    let current = SegmentId::new(*current_ts).expect("current segment id");
    write_segment(
        journal,
        owner,
        SegmentAddress::new(current).expect("current address"),
    )
    .expect("finish current segment");
    journal.reset().expect("reset current journal");
    *current_ts += 100_000;
    let next = SegmentId::new(*current_ts).expect("next segment id");
    journal
        .append(next, &encoded_load_part(&[(*current_ts, 2.0)]))
        .expect("append next segment");
}

#[test]
fn equal_endpoints_cover_the_complete_named_second() {
    let directory = tempfile::tempdir().expect("fixture directory");
    append_load_segment(
        directory.path(),
        &[
            (BASE - 1, 1.0),
            (BASE, 2.0),
            (BASE + 999_999, 3.0),
            (BASE + 1_000_000, 4.0),
        ],
        true,
    );
    let reader = Reader::open(directory.path()).expect("open fixture reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut output,
    )
    .expect("slice equal endpoints");
    assert_eq!(summary.requested_from, BASE);
    assert_eq!(summary.requested_to_exclusive, BASE + MICROS_PER_SECOND);
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE - 1, BASE, BASE + 999_999, BASE + 1_000_000]
    );
}

#[test]
fn finished_segments_keep_in_range_rows_and_nearest_context_cohorts() {
    let directory = tempfile::tempdir().expect("fixture directory");
    append_load_segment(
        directory.path(),
        &[
            (BASE - 31_000_000, 0.0),
            (BASE - 20_000_000, 1.0),
            (BASE, 2.0),
        ],
        true,
    );
    append_load_segment(
        directory.path(),
        &[
            (BASE + 3_000_000, 3.0),
            (BASE + 9_999_999, 4.0),
            (BASE + 15_000_000, 5.0),
            (BASE + 40_000_000, 6.0),
        ],
        true,
    );
    let reader = Reader::open(directory.path()).expect("open fixture reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND + 9),
        &mut output,
    )
    .expect("slice finished segments");
    assert_eq!(summary.actual_min_ts, BASE - 20_000_000);
    assert_eq!(summary.actual_max_ts, BASE + 15_000_000);
    assert_eq!(summary.rows_written, 5);
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [
            BASE - 20_000_000,
            BASE,
            BASE + 3_000_000,
            BASE + 9_999_999,
            BASE + 15_000_000,
        ]
    );
}

#[test]
fn active_only_and_finished_active_boundary_use_the_same_selection() {
    let active_only = tempfile::tempdir().expect("active fixture");
    append_load_segment(
        active_only.path(),
        &[(BASE - 5_000_000, 1.0), (BASE + 2_000_000, 2.0)],
        false,
    );
    let reader = Reader::open(active_only.path()).expect("open active reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND + 9),
        &mut output,
    )
    .expect("slice active only");
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE - 5_000_000, BASE + 2_000_000]
    );

    let mixed = tempfile::tempdir().expect("mixed fixture");
    append_load_segment(mixed.path(), &[(BASE - 5_000_000, 1.0), (BASE, 2.0)], true);
    append_load_segment(
        mixed.path(),
        &[(BASE + 2_000_000, 3.0), (BASE + 11_000_000, 4.0)],
        false,
    );
    let reader = Reader::open(mixed.path()).expect("open mixed reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND + 9),
        &mut output,
    )
    .expect("slice finished and active");
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE - 5_000_000, BASE, BASE + 2_000_000, BASE + 11_000_000]
    );
}

#[test]
fn rows_appended_after_capture_are_not_observed() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let segment_id = SegmentId::new(BASE).expect("segment id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal
        .append(segment_id, &encoded_load_part(&[(BASE, 1.0)]))
        .expect("append initial part");
    let later = encoded_load_part(&[(BASE + 1_000_000, 2.0)]);
    let reader = Reader::open(directory.path()).expect("open reader");
    let range = whole_second_range(BASE_SECOND, BASE_SECOND + 2)
        .micros()
        .expect("fixture range");
    let listing = reader
        .segments(range.context_from..range.context_to_exclusive)
        .expect("capture listing");
    journal
        .append(segment_id, &later)
        .expect("append after capture");
    let mut output = Vec::new();
    let summary = slice_captured_to_zms(&reader, range, &listing, &mut output)
        .expect("slice captured prefix");
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE]
    );
}

#[test]
fn one_active_rollover_restarts_the_complete_slice() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let segment_id = SegmentId::new(BASE).expect("segment id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal
        .append(segment_id, &encoded_load_part(&[(BASE, 1.0)]))
        .expect("append initial segment");
    let reader = Reader::open(directory.path()).expect("open reader");
    let calls = Rc::new(Counter::new(0_usize));
    let observed_calls = Rc::clone(&calls);
    let mut current_ts = BASE;
    let _hook = after_selection(move || {
        let call = observed_calls.get();
        observed_calls.set(call + 1);
        if call == 0 {
            finish_and_start_next(&mut journal, &owner, &mut current_ts);
        }
    });

    let mut scratch = tempfile::tempfile().expect("slice scratch");
    let mut output = Vec::new();
    let summary = super::slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut scratch,
        &mut output,
    )
    .expect("retry after active rollover");

    assert_eq!(calls.get(), 2);
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE, BASE + 100_000]
    );
}

#[test]
fn two_active_rollovers_fail_without_touching_output() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let segment_id = SegmentId::new(BASE).expect("segment id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal
        .append(segment_id, &encoded_load_part(&[(BASE, 1.0)]))
        .expect("append initial segment");
    let reader = Reader::open(directory.path()).expect("open reader");
    let calls = Rc::new(Counter::new(0_usize));
    let observed_calls = Rc::clone(&calls);
    let mut current_ts = BASE;
    let _hook = after_selection(move || {
        observed_calls.set(observed_calls.get() + 1);
        finish_and_start_next(&mut journal, &owner, &mut current_ts);
    });

    let mut scratch = tempfile::tempfile().expect("slice scratch");
    let mut output = b"caller bytes stay unchanged".to_vec();
    let error = super::slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut scratch,
        &mut output,
    )
    .expect_err("second active rollover must fail");

    assert_eq!(calls.get(), 2);
    assert!(matches!(
        error,
        SliceError::Reader(ref source) if source.source_changed_during_read()
    ));
    assert_eq!(output, b"caller bytes stay unchanged");
}

#[test]
fn a_finished_source_changed_after_capture_is_rejected() {
    let directory = tempfile::tempdir().expect("fixture directory");
    append_load_segment(directory.path(), &[(BASE, 1.0)], true);
    let zms = only_zms(directory.path());
    let reader = Reader::open(directory.path()).expect("open reader");
    let range = whole_second_range(BASE_SECOND, BASE_SECOND)
        .micros()
        .expect("fixture range");
    let listing = reader
        .segments(range.context_from..range.context_to_exclusive)
        .expect("capture listing");
    let file = OpenOptions::new()
        .write(true)
        .open(&zms)
        .expect("open fixture ZMS");
    file.write_all_at(b"X", 4).expect("change fixture ZMS");
    file.sync_all().expect("sync fixture change");
    let error = slice_captured_to_zms(&reader, range, &listing, &mut Vec::new())
        .expect_err("changed source must fail");
    assert!(matches!(error, SliceError::Reader(_)));
}

#[test]
fn an_empty_requested_range_and_a_corrupt_required_source_fail() {
    let empty = tempfile::tempdir().expect("empty fixture");
    append_load_segment(empty.path(), &[(BASE - 20_000_000, 1.0)], true);
    let reader = Reader::open(empty.path()).expect("open empty reader");
    assert!(matches!(
        slice_to_zms(
            &reader,
            whole_second_range(BASE_SECOND, BASE_SECOND),
            &mut Vec::new()
        ),
        Err(SliceError::NoRowsInRequestedRange)
    ));

    let corrupt = tempfile::tempdir().expect("corrupt fixture");
    append_load_segment(corrupt.path(), &[(BASE, 1.0)], true);
    let zms = only_zms(corrupt.path());
    let mut file = OpenOptions::new().write(true).open(zms).expect("open ZMS");
    file.write_all_at(b"X", 4).expect("corrupt body");
    file.flush().expect("flush corruption");
    let reader = Reader::open(corrupt.path()).expect("open corrupt reader");
    assert!(matches!(
        slice_to_zms(
            &reader,
            whole_second_range(BASE_SECOND, BASE_SECOND),
            &mut Vec::new()
        ),
        Err(SliceError::RequiredRangeUnreadable(_))
    ));
}

#[test]
fn a_corrupt_segment_starting_before_context_is_still_required_when_it_can_overlap() {
    let directory = tempfile::tempdir().expect("corrupt overlap fixture");
    append_load_segment(
        directory.path(),
        &[(BASE - 600_000_000, 1.0), (BASE, 2.0)],
        true,
    );
    let zms = only_zms(directory.path());
    let mut file = OpenOptions::new().write(true).open(zms).expect("open ZMS");
    file.write_all_at(b"X", 4).expect("corrupt body");
    file.flush().expect("flush corruption");
    append_load_segment(directory.path(), &[(BASE - 60_000_000, 3.0)], true);
    append_load_segment(directory.path(), &[(BASE, 4.0)], true);
    let reader = Reader::open(directory.path()).expect("open corrupt reader");
    assert!(matches!(
        slice_to_zms(
            &reader,
            whole_second_range(BASE_SECOND, BASE_SECOND),
            &mut Vec::new()
        ),
        Err(SliceError::RequiredRangeUnreadable(_))
    ));
}

#[test]
fn a_selected_corrupt_segment_is_required_even_when_its_id_follows_context() {
    let directory = tempfile::tempdir().expect("corrupt late-id fixture");
    append_load_segment(
        directory.path(),
        &[(BASE + 60_000_000, 1.0), (BASE, 2.0)],
        true,
    );
    let zms = only_zms(directory.path());
    let mut file = OpenOptions::new().write(true).open(zms).expect("open ZMS");
    file.write_all_at(b"X", 4).expect("corrupt body");
    file.flush().expect("flush corruption");
    append_load_segment(directory.path(), &[(BASE, 3.0)], true);
    let reader = Reader::open(directory.path()).expect("open corrupt reader");
    assert!(matches!(
        slice_to_zms(
            &reader,
            whole_second_range(BASE_SECOND, BASE_SECOND),
            &mut Vec::new()
        ),
        Err(SliceError::RequiredRangeUnreadable(_))
    ));
}

#[test]
fn a_corrupt_active_journal_fails_despite_readable_finished_rows() {
    let directory = tempfile::tempdir().expect("corrupt active fixture");
    append_load_segment(directory.path(), &[(BASE, 1.0)], true);
    append_load_segment(directory.path(), &[(BASE + 100_000, 2.0)], false);
    let active = directory.path().join("active.wal");
    let file = OpenOptions::new()
        .write(true)
        .open(active)
        .expect("open active journal");
    file.write_all_at(b"X", 0).expect("damage active header");
    file.sync_all().expect("sync active damage");
    let reader = Reader::open(directory.path()).expect("open corrupt active reader");

    let error = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut Vec::new(),
    )
    .expect_err("corrupt active journal must fail");
    assert!(matches!(
        error,
        SliceError::RequiredRangeUnreadable(warning)
            if matches!(
                *warning,
                StoreWarning {
                    affected: StoreObject::ActiveJournal,
                    reason: StoreWarningReason::ActiveJournal(_),
                    ..
                }
            )
    ));
}

#[test]
fn a_foreign_storage_entry_does_not_block_a_slice() {
    let directory = tempfile::tempdir().expect("foreign-entry fixture");
    append_load_segment(directory.path(), &[(BASE, 1.0)], true);
    std::fs::write(directory.path().join("operator-notes.txt"), b"unrelated")
        .expect("write foreign entry");
    let reader = Reader::open(directory.path()).expect("open reader");
    let range = whole_second_range(BASE_SECOND, BASE_SECOND)
        .micros()
        .expect("fixture range");
    let listing = reader
        .segments(range.context_from..range.context_to_exclusive)
        .expect("list storage with foreign entry");
    assert!(listing.warnings.iter().any(|warning| matches!(
        (warning.affected, warning.reason),
        (StoreObject::Foreign(_), StoreWarningReason::ForeignEntry(_))
    )));

    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut output,
    )
    .expect("foreign warning is ignorable");
    assert_eq!(
        timestamps_of(&output_segment(output, summary.segment_id), LOAD_TYPE),
        [BASE]
    );
}

#[test]
fn on_change_rows_keep_every_staggered_identity_inside_context() {
    let directory = tempfile::tempdir().expect("on-change fixture");
    let root = DataRoot::open(directory.path()).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let mut interner = Interner::new(DictLimits::default());
    let first_name = interner.intern(b"first-user").expect("intern first user");
    let second_name = interner.intern(b"second-user").expect("intern second user");
    let mut buffers = SectionBuffers::new();
    buffers.push(load(BASE, 1.0)).expect("buffer request row");
    buffers
        .push(OsUser {
            ts: Ts(BASE - 20_000_000),
            uid: 10,
            username: RegistryStrId(first_name.get()),
            scope: 0,
        })
        .expect("buffer first user");
    buffers
        .push(OsUser {
            ts: Ts(BASE - 10_000_000),
            uid: 20,
            username: RegistryStrId(second_name.get()),
            scope: 0,
        })
        .expect("buffer second user");
    let dictionaries = dict::encode(interner.window()).expect("encode user dictionary");
    let part = buffers
        .flush(&dictionaries)
        .expect("encode on-change fixture")
        .expect("nonempty on-change fixture");
    let id = SegmentId::new(BASE - 20_000_000).expect("fixture segment id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal.append(id, &part).expect("append fixture");
    write_segment(
        &journal,
        &owner,
        SegmentAddress::new(id).expect("fixture address"),
    )
    .expect("finish fixture");
    journal.reset().expect("reset fixture");
    drop(journal);
    drop(owner);

    let reader = Reader::open(directory.path()).expect("open reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut output,
    )
    .expect("slice staggered user rows");
    let segment = output_segment(output, summary.segment_id);
    assert_eq!(
        timestamps_of(&segment, USER_TYPE),
        [BASE - 20_000_000, BASE - 10_000_000]
    );
    let dictionary = segment.dictionary().expect("output dictionary");
    assert!(matches!(
        dictionary.resolve(first_name.get()),
        Some(Resolved::Str(b"first-user"))
    ));
    assert!(matches!(
        dictionary.resolve(second_name.get()),
        Some(Resolved::Str(b"second-user"))
    ));
}

#[test]
fn duplicate_rows_and_only_referenced_blob_dictionary_data_are_preserved() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("open fixture root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire fixture writer");
    let mut interner = Interner::new(DictLimits::new(4, 100).expect("small fixture limits"));
    let blob_bytes = vec![b'x'; 200];
    let selected = interner
        .intern_blob(&blob_bytes)
        .expect("intern selected blob");
    interner
        .intern(b"unreferenced-sentinel")
        .expect("intern unused string");
    let mut buffers = SectionBuffers::new();
    buffers.push(load(BASE, 1.0)).expect("first duplicate");
    buffers.push(load(BASE, 1.0)).expect("second duplicate");
    buffers
        .push(OsUser {
            ts: Ts(BASE),
            uid: 26,
            username: RegistryStrId(selected.get()),
            scope: 0,
        })
        .expect("buffer dictionary row");
    let dictionaries = dict::encode(interner.window()).expect("encode dictionaries");
    let part = buffers
        .flush(&dictionaries)
        .expect("encode fixture")
        .expect("fixture part");
    let id = SegmentId::new(BASE).expect("fixture id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal.append(id, &part).expect("append fixture");
    write_segment(
        &journal,
        &owner,
        SegmentAddress::new(id).expect("fixture address"),
    )
    .expect("finish fixture");
    journal.reset().expect("reset fixture");
    drop(journal);
    drop(owner);

    let reader = Reader::open(directory.path()).expect("open reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut output,
    )
    .expect("slice dictionary fixture");
    let segment = output_segment(output, summary.segment_id);
    assert_eq!(timestamps_of(&segment, LOAD_TYPE), [BASE, BASE]);
    assert_eq!(timestamps_of(&segment, USER_TYPE), [BASE]);
    let dictionary = segment.dictionary().expect("output dictionary");
    assert_eq!(dictionary.entries().count(), 1);
    assert!(matches!(
        dictionary.resolve(selected.get()),
        Some(Resolved::Blob(blob))
            if blob.full_len == blob_bytes.len() as u64
                && blob.truncated
                && blob.full_sha256.is_some()
    ));
}

#[test]
fn differing_metadata_rows_remain_and_the_report_path_accepts_the_slice() {
    let directory = tempfile::tempdir().expect("fixture directory");
    append_metadata_segment(directory.path(), BASE - 10_000_000, b"host-a", false);
    append_metadata_segment(directory.path(), BASE, b"host-b", true);
    append_metadata_segment(directory.path(), BASE + 10_000_000, b"host-c", false);
    let reader = Reader::open(directory.path()).expect("open reader");
    let mut output = Vec::new();
    let summary = slice_to_zms(
        &reader,
        whole_second_range(BASE_SECOND, BASE_SECOND),
        &mut output,
    )
    .expect("slice differing metadata");
    let segment = output_segment(output.clone(), summary.segment_id);
    assert_eq!(
        timestamps_of(&segment, METADATA_TYPE),
        [BASE - 10_000_000, BASE, BASE + 10_000_000]
    );
    let index = build(&segment).expect("production index accepts preserved metadata");
    for block in &index.blocks {
        match block {
            SeriesBlock::OsHealth(points) | SeriesBlock::OverallHealth(points) => {
                assert!(points.iter().all(|point| point.value.is_none()));
            }
            SeriesBlock::PostgresHealth(points) => {
                assert!(points.iter().all(|point| point.value.is_none()));
            }
            _ => {}
        }
    }
    let mut html = Vec::new();
    let report = write_html(
        HtmlReportInput {
            segment_id: summary.segment_id,
            max_zms_bytes: output.len() as u64,
            zms: output,
            visible_range: kronika_report::ReportTimeRange::new(
                summary.requested_from,
                summary.requested_to_exclusive,
            )
            .expect("slice report range"),
        },
        &mut html,
    )
    .expect("production report accepts slice");
    assert_eq!(report.segment_id, summary.segment_id);
    assert!(html.starts_with(b"<!doctype html>"));
    assert!(
        html.windows(26)
            .any(|window| window == b"__KRONIKA_REPORT_RUNTIME__")
    );
}

fn append_metadata_segment(root_path: &Path, ts: i64, hostname: &[u8], postgres: bool) {
    let root = DataRoot::open(root_path).expect("open metadata root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire metadata writer");
    let mut interner = Interner::new(DictLimits::default());
    let hostname = interner.intern(hostname).expect("intern hostname");
    let kernel = interner.intern(b"kernel").expect("intern kernel");
    let boot = interner
        .intern(if postgres { b"boot-b" } else { b"boot-a" })
        .expect("intern boot");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(ts),
            hostname: RegistryStrId(hostname.get()),
            kernel_version: RegistryStrId(kernel.get()),
            environment: if postgres {
                Environment::Container.as_u8()
            } else {
                Environment::Machine.as_u8()
            },
            clock_ticks_per_sec: if postgres { 250 } else { 100 },
            page_size_bytes: 4096,
            boot_id: RegistryStrId(boot.get()),
            btime: Ts(ts - 1_000_000),
            postgresql_enabled: postgres,
            postgresql_interval_seconds: if postgres { 30 } else { 0 },
            postgresql_effective_cpus: postgres.then_some(4),
        })
        .expect("buffer metadata");
    buffers.push(load(ts, 1.0)).expect("buffer metric");
    for resource in 0..3 {
        buffers
            .push(OsPsi {
                ts: Ts(ts),
                resource,
                some_avg10: 0.0,
                some_avg60: 0.0,
                some_avg300: 0.0,
                some_total: ts - BASE + i64::from(resource),
                full_avg10: None,
                full_avg60: None,
                full_avg300: None,
                full_total: None,
                scope: 0,
            })
            .expect("buffer PSI row");
    }
    let dictionaries = dict::encode(interner.window()).expect("encode metadata dictionary");
    let part = buffers
        .flush(&dictionaries)
        .expect("encode metadata part")
        .expect("metadata part");
    let id = SegmentId::new(ts).expect("metadata id");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    journal.append(id, &part).expect("append metadata part");
    write_segment(
        &journal,
        &owner,
        SegmentAddress::new(id).expect("metadata address"),
    )
    .expect("finish metadata segment");
    journal.reset().expect("reset metadata journal");
}

fn only_zms(root: &Path) -> PathBuf {
    fn visit(path: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(path).expect("read fixture directory") {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "zms") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(root, &mut found);
    assert_eq!(found.len(), 1);
    found.pop().expect("one fixture ZMS")
}
