use std::path::Path;

use kronika_format::{FRAME_HEADER_LEN, JOURNAL_HEADER_LEN, RESET_MARKER_LEN, validate_part};
use kronika_layout::{DataRoot, LayoutLimits, WriterOwner};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_source_log::pgbouncer::{Event, Level};
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_source_pg::archiver::ArchiverRow;
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::config::Config;
use crate::log_sources::{LogRows, PgBouncerBatch};
use crate::pg_sources::PgBatch;
use crate::scheduler::{DueSet, Intervals};
use crate::segments::{SegmentState, append_window_and_maybe_close, encode_window};
use crate::{append_pending_pg_batch, append_pending_window, scheduler::Scheduler};

const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_002;
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;
const PG_ARCHIVER_TYPE_ID: u32 = 1_008_001;
const PG_SETTINGS_TYPE_ID: u32 = 1_019_001;
const TEST_JOURNAL_MAX: usize = 64 * 1024;

fn config(root: &Path, journal_max_bytes: u64) -> Config {
    Config {
        storage_dir: root.to_path_buf(),
        tick_secs: 1,
        intervals: Intervals::default(),
        segment_max_bytes: u64::MAX,
        segment_max_age_secs: u64::MAX,
        journal_max_bytes,
        retention: None,
        pg_dsns: Vec::new(),
        postgres_effective_cpus: None,
        pg_logs: Vec::new(),
        pgbouncer_dsns: Vec::new(),
        pgbouncer_logs: Vec::new(),
    }
}

fn owner(root: &Path) -> WriterOwner {
    DataRoot::open(root)
        .expect("open data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer")
}

fn first_window(segment: &SegmentState) -> kronika_writer::FlushedPart {
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsLoadavg {
            ts: Ts(100),
            load1: 1.0,
            load5: 1.0,
            load15: 1.0,
            running: 1,
            total: 2,
            scope: 0,
        })
        .expect("buffer first row");
    encode_window(buffers, segment.interner()).expect("encode first window")
}

fn fill_journal_to_pressure(journal: &mut Journal, part: &kronika_writer::FlushedPart, max: usize) {
    let segment_id = journal.segment_id().expect("old segment id");
    let frame_len = FRAME_HEADER_LEN + part.body.len();
    while journal.bytes() + frame_len + RESET_MARKER_LEN <= max {
        journal
            .append(segment_id, &part.body)
            .expect("fill old journal near its cap");
    }
}

fn log_rows() -> LogRows {
    LogRows {
        postgres: Vec::new(),
        pgbouncer: vec![PgBouncerBatch {
            source_file: "/var/log/pgbouncer.log".to_owned(),
            events: vec![Event {
                ts: 200,
                level: Level::Error,
                database: Some("shop".to_owned()),
                username: Some("monitor".to_owned()),
                host: Some("127.0.0.1".to_owned()),
                text: "kernel file descriptor limit: 1024".to_owned(),
            }],
        }],
    }
}

fn archiver_batch() -> PgBatch {
    PgBatch::Archiver(ArchiverRow {
        ts: 200,
        archived_count: 3,
        last_archived_wal: Some("000000010000000000000001".to_owned()),
        last_archived_time: Some(190),
        failed_count: 0,
        last_failed_wal: None,
        last_failed_time: None,
        stats_reset: Some(100),
    })
}

fn settings_row() -> SettingsRow {
    SettingsRow {
        ts: 200,
        datid: 16_384,
        datname: "app".to_owned(),
        usesysid: 16_385,
        usename: "monitor".to_owned(),
        name: "shared_buffers".to_owned(),
        setting: "16384".to_owned(),
        unit: Some("8kB".to_owned()),
        source: "default".to_owned(),
        sourcefile: None,
        sourceline: None,
        pending_restart: false,
        context: "postmaster".to_owned(),
        vartype: "integer".to_owned(),
        boot_val: Some("16384".to_owned()),
        reset_val: Some("16384".to_owned()),
    }
}

fn assert_retained_batch_moves_to_fresh_segment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut segment = SegmentState::default();
    let first = first_window(&segment);
    let max = TEST_JOURNAL_MAX;
    let owner = owner(dir.path());
    let mut journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len: max,
            ..JournalConfig::default()
        },
    )
    .expect("open journal");
    let config = config(dir.path(), max as u64);
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append old segment row");
    fill_journal_to_pressure(&mut journal, &first, max);

    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();
    let outcome = append_pending_window(
        &mut journal,
        &owner,
        &config,
        false,
        &DueSet::logs(),
        &log_rows(),
        &[settings_row()],
        200,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    )
    .expect("retain and append log batch");

    assert!(outcome.accepted);
    assert!(outcome.appended);
    assert_eq!(outcome.written.len(), 1, "the old segment closes once");
    let old = std::fs::read(&outcome.written[0]).expect("read old segment");
    let old_catalog = validate_part(&old).expect("old segment is valid");
    assert!(
        old_catalog
            .entries
            .iter()
            .all(|entry| !matches!(entry.type_id, PGBOUNCER_TYPE_ID | PG_SETTINGS_TYPE_ID)),
        "the pending rows were not written to the old segment"
    );

    assert_eq!(journal.parts().len(), 1, "one retained batch is appended");
    let fresh = journal
        .read_part(journal.parts()[0])
        .expect("read fresh WAL part");
    let fresh_catalog = validate_part(&fresh).expect("fresh part and dictionary are valid");
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == PGBOUNCER_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == INSTANCE_METADATA_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1),
        "a fresh segment gets exactly one metadata row"
    );
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == PG_SETTINGS_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1),
        "the cached settings follow the retained window into the fresh segment"
    );
}

#[test]
fn journal_full_retains_and_reencodes_the_exact_log_batch() {
    assert_retained_batch_moves_to_fresh_segment();
}

#[test]
fn a_fresh_log_window_append_failure_is_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = owner(dir.path());
    let mut journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len: JOURNAL_HEADER_LEN,
            ..JournalConfig::default()
        },
    )
    .expect("open header-only journal");
    let config = config(dir.path(), JOURNAL_HEADER_LEN as u64);
    let mut segment = SegmentState::default();
    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();

    let error = match append_pending_window(
        &mut journal,
        &owner,
        &config,
        false,
        &DueSet::logs(),
        &log_rows(),
        &[],
        200,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    ) {
        Err(error) => error,
        Ok(_outcome) => {
            panic!("a fresh segment must not retry a deterministically rejected window")
        }
    };

    assert!(
        format!("{error:#}").contains("append the collection window to the journal"),
        "{error:#}"
    );
    assert!(journal.parts().is_empty());
}

fn assert_pg_batch_moves_to_fresh_segment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut segment = SegmentState::default();
    let first = first_window(&segment);
    let max = TEST_JOURNAL_MAX;
    let owner = owner(dir.path());
    let mut journal = Journal::open(
        &owner,
        JournalConfig {
            max_journal_len: max,
            ..JournalConfig::default()
        },
    )
    .expect("open journal");
    let config = config(dir.path(), max as u64);
    append_window_and_maybe_close(
        &mut journal,
        &owner,
        &config,
        &mut segment,
        100,
        false,
        &first,
    )
    .expect("append old segment row");
    fill_journal_to_pressure(&mut journal, &first, max);

    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();
    let outcome = append_pending_pg_batch(
        &mut journal,
        &owner,
        &config,
        false,
        &archiver_batch(),
        &[],
        200,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    )
    .expect("retain and append PostgreSQL batch");

    assert_eq!(outcome.written.len(), 1, "the old segment closes once");
    assert!(outcome.write.encoded_bytes > 0);
    assert!(outcome.write.wal_bytes_appended > outcome.write.encoded_bytes);
    let old = std::fs::read(&outcome.written[0]).expect("read old segment");
    let old_catalog = validate_part(&old).expect("old segment is valid");
    assert!(
        old_catalog
            .entries
            .iter()
            .all(|entry| entry.type_id != PG_ARCHIVER_TYPE_ID),
        "the pending PostgreSQL batch was not written to the old segment"
    );

    assert_eq!(journal.parts().len(), 1, "one retained batch is appended");
    let fresh = journal
        .read_part(journal.parts()[0])
        .expect("read fresh WAL part");
    let fresh_catalog = validate_part(&fresh).expect("fresh part and dictionary are valid");
    assert_eq!(
        fresh_catalog
            .entries
            .iter()
            .find(|entry| entry.type_id == PG_ARCHIVER_TYPE_ID)
            .map(|entry| entry.rows),
        Some(1)
    );
}

#[test]
fn journal_full_retains_and_reencodes_the_exact_postgres_batch() {
    assert_pg_batch_moves_to_fresh_segment();
}

#[test]
fn postgres_batch_is_not_repeated_in_incremental_log_windows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = owner(dir.path());
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let config = config(dir.path(), JournalConfig::default().max_journal_len as u64);
    let mut segment = SegmentState::default();
    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();

    append_pending_pg_batch(
        &mut journal,
        &owner,
        &config,
        false,
        &archiver_batch(),
        &[],
        200,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    )
    .expect("append PostgreSQL batch");
    for ts in [201, 202] {
        let outcome = append_pending_window(
            &mut journal,
            &owner,
            &config,
            false,
            &DueSet::logs(),
            &log_rows(),
            &[],
            ts,
            &mut process_io,
            &mut segment,
            &mut scheduler,
        )
        .expect("append incremental log batch");
        assert!(outcome.accepted);
    }

    assert_eq!(journal.parts().len(), 3);
    let mut postgres_rows = 0_u32;
    let mut log_rows = 0_u32;
    for (index, part) in journal.parts().iter().enumerate() {
        let bytes = journal.read_part(*part).expect("read WAL part");
        let catalog = validate_part(&bytes).expect("valid WAL part");
        let part_postgres_rows = catalog
            .entries
            .iter()
            .filter(|entry| entry.type_id == PG_ARCHIVER_TYPE_ID)
            .map(|entry| entry.rows)
            .sum::<u32>();
        if index == 0 {
            assert_eq!(part_postgres_rows, 1, "the PostgreSQL part has its row");
        } else {
            assert_eq!(
                part_postgres_rows, 0,
                "an incremental log window carries no PostgreSQL row"
            );
        }
        for entry in catalog.entries {
            if entry.type_id == PG_ARCHIVER_TYPE_ID {
                postgres_rows = postgres_rows.saturating_add(entry.rows);
            }
            if entry.type_id == PGBOUNCER_TYPE_ID {
                log_rows = log_rows.saturating_add(entry.rows);
            }
        }
    }
    assert_eq!(postgres_rows, 1, "the PostgreSQL snapshot appears once");
    assert_eq!(log_rows, 2, "both incremental log batches are present");
}

#[test]
fn cached_settings_are_added_once_when_logs_open_a_segment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = owner(dir.path());
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let config = config(dir.path(), JournalConfig::default().max_journal_len as u64);
    let mut segment = SegmentState::default();
    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();
    let settings = [settings_row()];

    for ts in [200, 201] {
        let outcome = append_pending_window(
            &mut journal,
            &owner,
            &config,
            false,
            &DueSet::logs(),
            &log_rows(),
            &settings,
            ts,
            &mut process_io,
            &mut segment,
            &mut scheduler,
        )
        .expect("append log window");
        assert!(outcome.accepted);
    }

    assert_eq!(journal.parts().len(), 2);
    for (index, part) in journal.parts().iter().enumerate() {
        let bytes = journal.read_part(*part).expect("read WAL part");
        let catalog = validate_part(&bytes).expect("valid WAL part");
        let settings_rows = catalog
            .entries
            .iter()
            .filter(|entry| entry.type_id == PG_SETTINGS_TYPE_ID)
            .map(|entry| entry.rows)
            .sum::<u32>();
        assert_eq!(settings_rows, u32::from(index == 0));
    }
}
