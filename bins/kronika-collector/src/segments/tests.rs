use std::fs;
use std::path::Path;

use kronika_format::{
    DictLimits, DictStats, FRAME_HEADER_LEN, JOURNAL_HEADER_LEN, PartMeta, RESET_MARKER_LEN,
    SectionInput, build_part, validate_part,
};
use kronika_layout::{DataRoot, FileKind, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_reader::{Cell, Reader, Resolved, SegmentKind};
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV2;
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV2;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::{DICT_STRINGS_TYPE_ID, SECTION_WRITE_BATCH_ROWS, Section, StrId, Ts};
use kronika_source_os::PasswdSnapshot;
use kronika_writer::{FlushedPart, Interner, Journal, JournalConfig, SectionBuffers};

use crate::config::Config;
use crate::scheduler::Intervals;

use super::open::{open_collector_journal, write_recovered_journal};
use super::{
    CloseConditions, SegmentState, append_window_and_maybe_close, close_open_segment, close_reason,
    encode_window,
};

fn empty_interner() -> Interner {
    Interner::new(DictLimits::new(8, 64).expect("test dictionary limits are valid"))
}

fn loadavg(ts: i64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(ts),
        load1: 1.5,
        load5: 1.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn flushed_window(ts: i64) -> FlushedPart {
    let mut buffers = SectionBuffers::new();
    buffers.push(loadavg(ts)).expect("one row fits");
    buffers
        .flush_with_summary(&[])
        .expect("window encodes")
        .expect("one row yields one part")
}

fn passwd_snapshot() -> PasswdSnapshot {
    let file = tempfile::NamedTempFile::new().expect("create passwd fixture");
    fs::write(
        file.path(),
        b"postgres:x:26:26::/var/lib/postgresql:/bin/false\n",
    )
    .expect("write passwd fixture");
    PasswdSnapshot::read(file.path()).expect("read passwd fixture")
}

fn user_window(segment: &mut SegmentState, ts: i64) -> (FlushedPart, Vec<(u8, u32)>) {
    let (interner, users) = segment.os_state_mut();
    let (rows, pending) = users.prepare_rows(interner, 0, ts, [26, 26]);
    assert_eq!(rows.len(), 1);
    let mut buffers = SectionBuffers::new();
    for row in rows {
        buffers.push(row).expect("buffer user reference");
    }
    let flushed = encode_window(buffers, segment.interner()).expect("encode user reference");
    (flushed, pending)
}

fn cgroup_v2_window() -> FlushedPart {
    let mut interner = empty_interner();
    let path_id = interner.intern(b"/m1").expect("intern cgroup path");
    let section_path_id = StrId(path_id.get());
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsCgroupCpuV2 {
            ts: Ts(101),
            cgroup_path: section_path_id,
            usage_usec: 1_000,
            user_usec: 600,
            system_usec: 400,
            throttled_usec: 70,
            nr_throttled: 2,
            quota_usec: 200_000,
            period_usec: 100_000,
            cpuset_cpus: Some(2),
            scope: 3,
        })
        .expect("buffer cgroup CPU compatibility row");
    buffers
        .push(OsCgroupMemoryV2 {
            ts: Ts(102),
            cgroup_path: section_path_id,
            current: 1024,
            max: Some(2048),
            anon: 100,
            file: 200,
            kernel: 30,
            slab: 20,
            shmem: 64,
            low_events: 1,
            high_events: 2,
            max_events: 3,
            oom_events: 4,
            oom_kill: 5,
            scope: 3,
        })
        .expect("buffer cgroup memory compatibility row");
    let dictionary = kronika_writer::dict::encode(interner.window()).expect("encode dictionary");
    buffers
        .flush_with_summary(&dictionary)
        .expect("encode compatibility part")
        .expect("compatibility rows yield a part")
}

fn test_config(storage_dir: &Path) -> Config {
    Config {
        storage_dir: storage_dir.to_path_buf(),
        tick_secs: 5,
        intervals: Intervals::default(),
        segment_max_bytes: u64::MAX,
        segment_max_age_secs: u64::MAX,
        journal_max_bytes: u64::MAX,
        retention: None,
        pg_dsns: Vec::new(),
        postgres_effective_cpus: None,
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn open_journal(root_path: &Path, max_journal_len: usize) -> (WriterOwner, Journal) {
    let root = DataRoot::open(root_path).expect("open test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire test writer");
    let journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len,
            ..JournalConfig::default()
        },
    )
    .expect("open test journal");
    (owner, journal)
}

fn one_part_journal_cap(part_len: usize) -> usize {
    JOURNAL_HEADER_LEN + FRAME_HEADER_LEN + part_len + RESET_MARKER_LEN
}

fn segment_path(owner: &WriterOwner, ts: i64) -> std::path::PathBuf {
    let id = SegmentId::new(ts).expect("test timestamp is a valid segment id");
    let address = SegmentAddress::new(id).expect("test segment has a UTC address");
    owner.root().diagnostic_file_path(address, FileKind::Zms)
}

#[test]
fn journal_full_writes_accumulated_segment_and_defers_window() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    let incoming = flushed_window(200);
    let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("the first frame is exempt from the journal cap");

    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &incoming,
    )
    .expect("full journal writes the accumulated segment");

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, "journal-full");
    assert!(journal.parts().is_empty());
    assert_eq!(segment.first_ts(), None);
}

#[test]
fn configured_size_closes_only_after_the_valid_frame_is_appended() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(dir.path());
    config.segment_max_bytes = 1;
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut segment = SegmentState::default();
    let window = flushed_window(100);

    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &window,
    )
    .expect("append first and only then close for size");

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, "size");
    assert!(journal.parts().is_empty());
    let reader = Reader::open(dir.path()).expect("open production reader");
    let listing = reader.segments(..).expect("list size-closed segment");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 1);
    let stored = reader
        .open_segment(&listing.segments[0])
        .expect("open size-closed segment");
    assert_eq!(stored.rows_of(OsLoadavg::CONTRACT.type_id.get()), Some(1));
}

#[test]
fn configured_sixty_four_mib_boundary_is_exact() {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let limit_bytes = usize::try_from(LIMIT).expect("64 MiB fits usize");
    let conditions = CloseConditions {
        forced: false,
        age_expired: false,
    };

    assert_eq!(close_reason(conditions, limit_bytes - 1, LIMIT), None);
    assert_eq!(close_reason(conditions, limit_bytes, LIMIT), Some("size"));
    assert_eq!(
        close_reason(conditions, limit_bytes + 1, LIMIT),
        Some("size")
    );
}

#[test]
fn aggregate_rows_cross_internal_batches_without_early_rotation() {
    const FIRST_TS: i64 = 1_700_000_000_000_000;
    const TOTAL_ROWS: usize = SECTION_WRITE_BATCH_ROWS + 1;

    let dir = tempfile::tempdir().expect("tempdir");
    let config = test_config(dir.path());
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut segment = SegmentState::default();
    let mut first_buffers = SectionBuffers::new();
    for row in 0..SECTION_WRITE_BATCH_ROWS {
        first_buffers
            .push(loadavg(
                FIRST_TS + i64::try_from(row).expect("test row fits i64"),
            ))
            .expect("one internal batch fits");
    }
    let first = encode_window(first_buffers, segment.interner()).expect("encode first batch");
    assert!(
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            FIRST_TS,
            false,
            &first,
        )
        .expect("append first batch")
        .is_empty()
    );

    let second = flushed_window(
        FIRST_TS + i64::try_from(SECTION_WRITE_BATCH_ROWS).expect("batch rows fit i64"),
    );
    assert!(
        append_window_and_maybe_close(
            &mut journal,
            &owner,
            &config,
            &mut segment,
            FIRST_TS + 1,
            false,
            &second,
        )
        .expect("append row beyond one internal batch")
        .is_empty()
    );
    assert_eq!(journal.parts().len(), 2);

    let path = close_open_segment(&mut journal, &owner, &mut segment, "test-end")
        .expect("finish aggregate section");
    let finished = fs::read(&path).expect("read finished segment");
    let catalog = validate_part(&finished).expect("finished segment is canonical");
    let entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == OsLoadavg::CONTRACT.type_id.get())
        .expect("loadavg section is present");
    assert_eq!(entry.rows as usize, TOTAL_ROWS);

    let reader = Reader::open(dir.path()).expect("open production reader");
    let listing = reader.segments(..).expect("list finished segment");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 1);
    let stored = reader
        .open_segment(&listing.segments[0])
        .expect("open finished segment");
    let rows = stored
        .rows(OsLoadavg::CONTRACT.type_id.get())
        .expect("decode aggregate section");
    assert_eq!(rows.len(), TOTAL_ROWS);
}

#[test]
fn invalid_part_at_journal_cap_is_transactional() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let first = flushed_window(100);
    let (owner, mut journal) = open_journal(dir.path(), one_part_journal_cap(first.body.len()));
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append first");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let first_before = segment.first_ts();
    let dictionary_before = segment.interner.stats();
    let invalid = FlushedPart {
        body: b"not a ZMS part".to_vec(),
        summary: flushed_window(200).summary,
    };

    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &invalid,
    )
    .expect_err("invalid incoming part is rejected before a full-journal write");

    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(segment.first_ts(), first_before);
    assert_eq!(segment.interner.stats(), dictionary_before);
    assert_eq!(journal.parts().len(), 1);
    assert!(!segment_path(&owner, 100).exists());
}

#[test]
fn persistent_interner_writes_only_new_dictionary_entries_to_each_part() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = test_config(dir.path());
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut segment = SegmentState::default();

    segment
        .interner_mut()
        .intern(b"shared-value")
        .expect("intern first value");
    let mut first_buffers = SectionBuffers::new();
    first_buffers.push(loadavg(100)).expect("buffer first row");
    let first = encode_window(first_buffers, segment.interner()).expect("encode first window");
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append first window");

    segment
        .interner_mut()
        .intern(b"shared-value")
        .expect("re-intern shared value");
    let mut second_buffers = SectionBuffers::new();
    second_buffers
        .push(loadavg(200))
        .expect("buffer second row");
    let second = encode_window(second_buffers, segment.interner()).expect("encode second window");
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        200,
        false,
        &second,
    )
    .expect("append second window");

    let first_part = journal
        .read_part(journal.parts()[0])
        .expect("read first WAL part");
    let first_catalog = validate_part(&first_part).expect("validate first WAL part");
    assert!(
        first_catalog
            .entries
            .iter()
            .any(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
    );
    let second_part = journal
        .read_part(journal.parts()[1])
        .expect("read second WAL part");
    let second_catalog = validate_part(&second_part).expect("validate second WAL part");
    assert!(
        second_catalog
            .entries
            .iter()
            .all(|entry| entry.type_id != DICT_STRINGS_TYPE_ID)
    );

    let dest = close_open_segment(&mut journal, &owner, &mut segment, "test")
        .expect("write reconstructed segment");
    let finished = fs::read(dest).expect("read reconstructed segment");
    let finished_catalog = validate_part(&finished).expect("validate reconstructed segment");
    assert_eq!(
        finished_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
    assert_eq!(
        finished_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == OsLoadavg::CONTRACT.type_id.get())
            .map(|entry| entry.rows),
        Some(2)
    );
}

#[test]
fn failed_close_drops_segment_memory_and_preserves_the_journal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let config = test_config(dir.path());
    let mut segment = SegmentState::default();
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = flushed_window(100);
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &part,
    )
    .expect("append one window");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let destination = segment_path(&owner, 100);
    fs::create_dir_all(destination.parent().expect("segment has a parent"))
        .expect("create segment day");
    fs::write(&destination, b"conflicting segment").expect("write conflicting segment");

    close_open_segment(&mut journal, &owner, &mut segment, "test")
        .expect_err("a conflicting destination stops close");

    assert!(segment.is_empty());
    assert_eq!(segment.interner.stats(), DictStats::default());
    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(journal.parts().len(), 1);
}

#[test]
fn recovery_preserves_an_unreadable_journal_at_its_canonical_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let bytes = b"not a valid journal";
    fs::write(&path, bytes).expect("write unreadable journal");
    let root = DataRoot::open(dir.path()).expect("open test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire test writer");
    let journal_max_bytes =
        u64::try_from(JournalConfig::default().max_journal_len).expect("journal cap fits u64");

    let err = open_collector_journal(&owner, journal_max_bytes)
        .expect_err("an unreadable journal stops recovery");

    assert!(format!("{err:#}").contains("existing file is preserved"));
    assert_eq!(fs::read(&path).expect("read preserved journal"), bytes);
    assert!(!dir.path().join("active.wal.damaged").exists());
}

#[test]
fn recovery_publication_failure_keeps_the_readable_journal_canonical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = flushed_window(100);
    journal
        .append(
            SegmentId::new(100).expect("valid recovery identity"),
            &part.body,
        )
        .expect("append readable part");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");
    let destination = segment_path(&owner, 100);
    fs::create_dir_all(destination.parent().expect("segment has a parent"))
        .expect("create segment day");
    fs::write(&destination, b"conflicting segment").expect("write conflicting segment");

    write_recovered_journal(&mut journal, &owner)
        .expect_err("a conflicting destination stops recovery");

    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(
        fs::read(destination).expect("read existing segment"),
        b"conflicting segment"
    );
    assert_eq!(journal.parts().len(), 1);
}

#[test]
fn recovery_publishes_persisted_cgroup_v2_rows() {
    const CPU_V2_TYPE_ID: u32 = 1_201_002;
    const MEMORY_V2_TYPE_ID: u32 = 1_202_002;

    let dir = tempfile::tempdir().expect("tempdir");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = cgroup_v2_window();
    journal
        .append(
            SegmentId::new(100).expect("valid recovery identity"),
            &part.body,
        )
        .expect("persist compatibility part");
    drop(journal);
    drop(owner);

    let (owner, journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let persisted_part = journal
        .read_part(*journal.parts().first().expect("persisted journal part"))
        .expect("read persisted journal part");
    let persisted_catalog = validate_part(&persisted_part).expect("validate persisted part");
    assert!(
        persisted_catalog
            .entries
            .iter()
            .any(|entry| entry.type_id == CPU_V2_TYPE_ID)
    );
    assert!(
        persisted_catalog
            .entries
            .iter()
            .any(|entry| entry.type_id == MEMORY_V2_TYPE_ID)
    );
    drop(journal);
    drop(owner);

    let root = DataRoot::open(dir.path()).expect("reopen test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("reacquire test writer");
    let journal_max_bytes =
        u64::try_from(JournalConfig::default().max_journal_len).expect("journal cap fits u64");
    let (journal, recovered_path) =
        open_collector_journal(&owner, journal_max_bytes).expect("recover compatibility layouts");
    let recovered_path = recovered_path.expect("nonempty journal writes a segment");
    assert!(journal.parts().is_empty());
    assert_eq!(
        fs::metadata(dir.path().join("active.wal"))
            .expect("stat reset journal")
            .len(),
        JOURNAL_HEADER_LEN as u64
    );

    let reader = Reader::open(dir.path()).expect("open production reader");
    let listing = reader.segments(..).expect("list recovered segment");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(listing.segments[0].kind(), SegmentKind::Finished);
    let recovered = reader
        .open_segment(&listing.segments[0])
        .expect("open recovered segment");
    assert_eq!(
        recovered.source_label(),
        recovered_path.display().to_string()
    );
    assert_eq!(recovered.rows_of(CPU_V2_TYPE_ID), Some(1));
    assert_eq!(recovered.rows_of(MEMORY_V2_TYPE_ID), Some(1));

    let cpu_rows = recovered
        .rows(CPU_V2_TYPE_ID)
        .expect("decode recovered cgroup CPU rows");
    let memory_rows = recovered
        .rows(MEMORY_V2_TYPE_ID)
        .expect("decode recovered cgroup memory rows");
    assert_eq!(cpu_rows[0].get("cpuset_cpus"), Some(&Cell::I64(2)));
    assert_eq!(memory_rows[0].get("shmem"), Some(&Cell::I64(64)));
    let Some(Cell::StrId(cpu_path_id)) = cpu_rows[0].get("cgroup_path") else {
        panic!("cgroup CPU path must remain a dictionary id")
    };
    let Some(Cell::StrId(memory_path_id)) = memory_rows[0].get("cgroup_path") else {
        panic!("cgroup memory path must remain a dictionary id")
    };
    assert_eq!(cpu_path_id, memory_path_id);
    let recovered_dictionary = recovered.dictionary().expect("decode recovered dictionary");
    assert_eq!(
        recovered_dictionary.resolve(*cpu_path_id),
        Some(Resolved::Str(b"/m1"))
    );
}

#[test]
fn user_reference_survives_recovery_and_is_reemitted_after_forced_rollover() {
    const USER_TYPE_ID: u32 = 1_124_002;
    const FIRST_TS: i64 = 1_700_000_000_000_000;
    const SECOND_TS: i64 = FIRST_TS + 1_000_000;

    let dir = tempfile::tempdir().expect("tempdir");
    let snapshot = passwd_snapshot();
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let config = test_config(dir.path());
    let mut segment = SegmentState::with_user_snapshot(snapshot.clone());
    let (first, pending) = user_window(&mut segment, FIRST_TS);
    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        FIRST_TS,
        false,
        &first,
    )
    .expect("append first user reference");
    assert!(finished.is_empty());
    segment.mark_users_recorded(&pending);
    drop(journal);
    drop(owner);

    let root = DataRoot::open(dir.path()).expect("reopen test data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("reacquire test writer");
    let journal_max_bytes =
        u64::try_from(JournalConfig::default().max_journal_len).expect("journal cap fits u64");
    let (mut journal, recovered_path) =
        open_collector_journal(&owner, journal_max_bytes).expect("recover user reference");
    assert!(recovered_path.is_some());
    assert!(journal.parts().is_empty());

    let mut segment = SegmentState::with_user_snapshot(snapshot);
    let (second, pending) = user_window(&mut segment, SECOND_TS);
    let finished = append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        SECOND_TS,
        true,
        &second,
    )
    .expect("append and close second user reference");
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, "forced");
    assert!(segment.is_empty());
    assert_eq!(pending, [(0, 26)]);

    let reader = Reader::open(dir.path()).expect("open production reader");
    let listing = reader.segments(..).expect("list user segments");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 2);
    for descriptor in &listing.segments {
        assert_eq!(descriptor.kind(), SegmentKind::Finished);
        let stored = reader.open_segment(descriptor).expect("open user segment");
        let rows = stored.rows(USER_TYPE_ID).expect("decode user rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("uid"), Some(&Cell::U32(26)));
        let Some(Cell::StrId(username)) = rows[0].get("username") else {
            panic!("user name must remain a dictionary id");
        };
        let dictionary = stored.dictionary().expect("decode user dictionary");
        assert_eq!(
            dictionary.resolve(*username),
            Some(Resolved::Str(b"postgres"))
        );
    }
}

#[test]
fn recovery_preserves_a_populated_part_without_a_timestamp() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let body = OsLoadavg::encode(&[loadavg(100)]).expect("encode section");
    let part = build_part(
        &[SectionInput {
            type_id: OsLoadavg::CONTRACT.type_id.get(),
            rows: 1,
            body: &body,
        }],
        PartMeta {
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        },
    );
    journal
        .append(SegmentId::new(100).expect("valid recovery identity"), &part)
        .expect("append structurally valid part");
    let bytes_before = fs::read(&path).expect("snapshot active.wal");

    let err = write_recovered_journal(&mut journal, &owner)
        .expect_err("populated sentinel-timestamp part is not publishable");

    assert!(format!("{err:#}").contains("active.wal is preserved"));
    assert_eq!(fs::read(&path).expect("read active.wal"), bytes_before);
    assert_eq!(journal.parts().len(), 1);
    assert!(
        fs::read_dir(dir.path())
            .expect("read storage directory")
            .all(|entry| {
                entry.expect("directory entry").path().extension()
                    != Some(std::ffi::OsStr::new("zms"))
            })
    );
}

#[test]
fn recovery_publishes_a_readable_journal_without_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("active.wal");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let part = build_part(
        &[],
        PartMeta {
            min_ts: i64::MAX,
            max_ts: i64::MIN,
        },
    );
    journal
        .append(SegmentId::new(100).expect("valid recovery identity"), &part)
        .expect("append empty but structurally valid part");

    let dest = write_recovered_journal(&mut journal, &owner)
        .expect("publish the readable journal")
        .expect("a nonempty journal gets a publication attempt");

    let recovered = fs::read(dest).expect("read recovered segment");
    let catalog = validate_part(&recovered).expect("recovered segment is valid");
    assert!(catalog.entries.is_empty());
    assert_eq!((catalog.min_ts, catalog.max_ts), (0, 0));
    assert!(journal.parts().is_empty());
    assert_eq!(
        fs::metadata(path).expect("stat reset journal").len(),
        JOURNAL_HEADER_LEN as u64
    );
}

#[test]
fn recovery_publishes_a_valid_dictionary_only_journal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (owner, mut journal) = open_journal(dir.path(), JournalConfig::default().max_journal_len);
    let mut interner = empty_interner();
    interner.intern(b"dict").expect("intern dictionary value");
    let dictionary = kronika_writer::dict::encode(interner.window()).expect("encode dictionary");
    let part = SectionBuffers::new()
        .flush_with_summary(&dictionary)
        .expect("encode dictionary-only part")
        .expect("dictionary yields a part");
    journal
        .append(
            SegmentId::new(100).expect("valid recovery identity"),
            &part.body,
        )
        .expect("append dictionary-only part");

    let dest = write_recovered_journal(&mut journal, &owner)
        .expect("publish dictionary-only journal")
        .expect("a nonempty journal gets a publication attempt");

    let recovered = fs::read(dest).expect("read recovered segment");
    let catalog = validate_part(&recovered).expect("recovered segment is valid");
    assert_eq!((catalog.min_ts, catalog.max_ts), (0, 0));
    assert_eq!(
        catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
}
