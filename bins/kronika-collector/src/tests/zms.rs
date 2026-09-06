use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kronika_format::{ENTRY_LEN, FRAME_HEADER_LEN, JOURNAL_HEADER_LEN};
use kronika_layout::{DataRoot, LayoutLimits, WriterOwner};
use kronika_reader::{Cell, Reader, Resolved, Row, SegmentKind};
use kronika_registry::os_block_topology::OsBlockTopology;
use kronika_registry::os_cgroup_context::OsCgroupContext;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_cgroup_io::OsCgroupIo;
use kronika_registry::os_cgroup_memory::OsCgroupMemory;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::os_cpufreq::{OsCpufreq, OsCpufreqPolicy};
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::{DICT_STRINGS_TYPE_ID, PgWalStorage, StrId, Ts, section_name};
use kronika_source_os::PasswdSnapshot;
use kronika_source_os::passwd::MAX_PASSWD_BYTES;
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_source_os::{SysFs, block_topology, cgroup, cpufreq};
use kronika_source_pg::activity::{ActivityRow, ActivityVersion};
use kronika_source_pg::query::{BATCH_LOGICAL_BYTES, BATCH_ROWS};
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::statements::{StatementsRow, StatementsVersion};
use kronika_source_pg::store_plans::VadvRow;
use kronika_source_pg::user_indexes::{UserIndexesRow, UserIndexesVersion};
use kronika_source_pg::user_tables::{UserTablesRow, UserTablesVersion};
use kronika_writer::{Journal, JournalConfig, SectionBuffers};

use crate::append_pending_pg_batch;
use crate::config::Config;
use crate::logging::peak_rss_kib;
use crate::os_sources::{OsSources, UserReferences, push_os_sources};
use crate::pg_sources::{PgBatch, push_pg_batch};
use crate::scheduler::{Intervals, Scheduler};
use crate::segments::{
    SegmentState, append_window_and_maybe_close, close_open_segment, encode_window,
};

const STATEMENT_BATCH_COUNT: usize = 20;
const BASE_QUERY_COUNT: usize = 160;
const UNIQUE_QUERY_COUNT: usize = 40;
const QUERY_BYTES: usize = 2_048;
const STATEMENT_ROW_OVERHEAD_BOUND: usize = 512;
const PLAN_BATCH_COUNT: usize = 3;
const PLAN_ROWS_PER_BATCH: usize = 120;
const PLAN_BYTES: usize = 4_096;
const PLAN_ROW_OVERHEAD_BYTES: usize = 274;
const PG_STAT_STATEMENTS_V6_TYPE_ID: u32 = 1_002_006;
const PG_STORE_PLANS_VADV_TYPE_ID: u32 = 1_004_001;
const PG_WAL_STORAGE_TYPE_ID: u32 = 1_020_001;
const PG_STAT_ACTIVITY_V3_TYPE_ID: u32 = 1_001_004;
const CGROUP_CONTEXT_TYPE_ID: u32 = 1_205_001;
const CGROUP_CPU_TYPE_ID: u32 = 1_201_001;
const CGROUP_MEMORY_TYPE_ID: u32 = 1_202_001;
const CGROUP_IO_TYPE_ID: u32 = 1_203_002;
const CGROUP_PIDS_TYPE_ID: u32 = 1_204_001;
const CGROUP_SNAPSHOTS_PER_HOUR: usize = 120;
const CGROUP_CANDIDATE_COUNT: usize = cgroup::MAX_CGROUP_CANDIDATES;
const CGROUP_DEVICES_PER_CANDIDATE: usize = cgroup::MAX_CGROUP_IO_ROWS / CGROUP_CANDIDATE_COUNT;
const CGROUP_COST_CHILD_ENV: &str = "KRONIKA_CGROUP_COST_CHILD";
const CGROUP_COST_TEST: &str =
    "tests::zms::bounded_cgroup_hour_reports_collection_and_production_writer_costs";
const CPUFREQ_POLICY_TYPE_ID: u32 = 1_121_001;
const CPUFREQ_TYPE_ID: u32 = 1_122_001;
const MOUNTINFO_TYPE_ID: u32 = 1_112_002;
const BLOCK_TOPOLOGY_TYPE_ID: u32 = 1_123_001;
const USER_TYPE_ID: u32 = 1_124_002;
const USER_COST_CHILD_ENV: &str = "KRONIKA_USER_COST_CHILD";
const USER_COST_TEST: &str =
    "tests::zms::process_user_references_report_production_storage_and_resource_costs";
const WAL_STORAGE_SNAPSHOTS_PER_HOUR: usize = 120;
const ACTIVITY_SNAPSHOTS_PER_HOUR: usize = 120;
const ACTIVITY_ROWS_PER_SNAPSHOT: usize = 64;
const CGROUP_CONTEXT_SNAPSHOTS_PER_HOUR: usize = 360;
const CPUFREQ_SNAPSHOTS_PER_HOUR: usize = 360;
const CPUFREQ_POLICY_COUNT: usize = 128;
const CPUFREQ_COST_CHILD_ENV: &str = "KRONIKA_CPUFREQ_COST_CHILD";
const CPUFREQ_COST_TEST: &str =
    "tests::zms::cpufreq_hour_reports_collection_and_production_writer_costs";
const STORAGE_SNAPSHOTS_PER_HOUR: usize = 60;
const STORAGE_MOUNTS_PER_SNAPSHOT: usize = 64;
const STORAGE_EDGES_PER_SNAPSHOT: usize = 128;
const RELATION_REPRESENTATIVE_TABLES: usize = 1_000;
const RELATION_REPRESENTATIVE_INDEXES: usize = 2_000;
const RELATION_STRESS_OBJECTS: usize = 5_000;
const RELATION_SNAPSHOTS: usize = 12;
const RELATION_COST_PROFILE_ENV: &str = "KRONIKA_RELATION_COST_PROFILE";
const RELATION_COST_TEST: &str =
    "tests::zms::relation_tablespace_layouts_report_production_writer_costs";
const BASE_TS: i64 = 1_700_000_000_000_000;
const PRE_CHANGE_ZMS_BYTES: u64 = 3_141_820;
const PRE_CHANGE_DICT_STRINGS_BYTES: u64 = 1_978_136;
const MAX_REPLAY_ZMS_BYTES: u64 = 2_800_000;
const MAX_REPLAY_WAL_BYTES: u64 = 3_400_000;
const MAX_REPLAY_DICT_STRINGS_BYTES: u64 = 1_600_000;
const MAX_REPLAY_DICT_BLOBS_BYTES: u64 = 1_200_000;
const QUERY_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";

#[derive(Debug, Default)]
struct ReplayWriteReport {
    paths: Vec<PathBuf>,
    close_reasons: BTreeMap<&'static str, usize>,
    cumulative_appended_wal_bytes: u64,
    peak_wal_bytes: usize,
    appended_windows: usize,
    max_batch_rows: usize,
    max_batch_logical_bytes: usize,
}

#[derive(Debug, Default)]
struct SectionReport {
    rows: u64,
    bytes: u64,
}

struct ReplayAppend<'a> {
    journal: &'a mut Journal,
    writer: &'a WriterOwner,
    config: &'a Config,
    segment: &'a mut SegmentState,
    settings: &'a [SettingsRow],
    report: &'a mut ReplayWriteReport,
}

struct ReplayArtifactReport {
    segments: usize,
    zms_bytes: u64,
    section_body_bytes: u64,
    overhead_bytes: u64,
    windows: u64,
    statement_rows: usize,
    plan_rows: usize,
    rss_kib: u64,
    sections: BTreeMap<&'static str, SectionReport>,
}

fn config(root: &Path, journal_max_bytes: u64) -> Config {
    Config {
        storage_dir: root.to_path_buf(),
        tick_secs: 1,
        intervals: Intervals::default(),
        segment_max_bytes: 64 * 1024 * 1024,
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
        .expect("open replay data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire replay writer")
}

fn self_cpu_ticks() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").expect("read test process stat");
    let after_comm = stat.rsplit_once(')').expect("process stat has comm").1;
    let mut fields = after_comm.split_whitespace();
    let user: u64 = fields
        .nth(11)
        .expect("process stat has user CPU")
        .parse()
        .expect("user CPU is numeric");
    let system: u64 = fields
        .next()
        .expect("process stat has system CPU")
        .parse()
        .expect("system CPU is numeric");
    user.saturating_add(system)
}

fn settings_row() -> SettingsRow {
    SettingsRow {
        ts: BASE_TS,
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

fn query_text(index: usize) -> String {
    let mut state = u64::try_from(index)
        .unwrap_or(u64::MAX)
        .wrapping_add(1)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut bytes = Vec::with_capacity(QUERY_BYTES);
    bytes.extend_from_slice(b"select '");
    while bytes.len() + 1 < QUERY_BYTES {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let random = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        bytes.push(QUERY_ALPHABET[(random & 63) as usize]);
    }
    bytes.push(b'\'');
    String::from_utf8(bytes).expect("query alphabet is ASCII")
}

fn statement_row(batch_index: usize, query_index: usize) -> StatementsRow {
    let sample = i64::try_from(batch_index).unwrap_or(i64::MAX);
    let sample_f64 = f64::from(u32::try_from(batch_index).unwrap_or(u32::MAX));
    let query_id = i64::try_from(query_index)
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    let ts = BASE_TS.saturating_add(sample);
    StatementsRow {
        ts,
        queryid: Some(query_id),
        userid: 16_385,
        dbid: 16_384,
        toplevel: Some(true),
        datname: Some("app".to_owned()),
        usename: Some("monitor".to_owned()),
        query: Some(query_text(query_index)),
        calls: sample.saturating_add(1),
        rows: query_id,
        plans: Some(sample),
        total_exec_time: sample_f64,
        total_plan_time: Some(0.0),
        min_exec_time: 0.0,
        max_exec_time: sample_f64,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: Some(0.0),
        max_plan_time: Some(0.0),
        mean_plan_time: Some(0.0),
        stddev_plan_time: Some(0.0),
        shared_blks_hit: sample,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        shared_blk_read_time: 0.0,
        shared_blk_write_time: 0.0,
        local_blk_read_time: Some(0.0),
        local_blk_write_time: Some(0.0),
        temp_blk_read_time: Some(0.0),
        temp_blk_write_time: Some(0.0),
        wal_records: Some(sample),
        wal_fpi: Some(0),
        wal_bytes: Some(sample),
        wal_buffers_full: Some(0),
        jit_functions: Some(0),
        jit_generation_time: Some(0.0),
        jit_inlining_count: Some(0),
        jit_inlining_time: Some(0.0),
        jit_optimization_count: Some(0),
        jit_optimization_time: Some(0.0),
        jit_emission_count: Some(0),
        jit_emission_time: Some(0.0),
        jit_deform_count: Some(0),
        jit_deform_time: Some(0.0),
        parallel_workers_to_launch: Some(0),
        parallel_workers_launched: Some(0),
        stats_since: Some(BASE_TS),
        minmax_stats_since: Some(BASE_TS),
    }
}

fn statement_batch(batch_index: usize) -> (PgBatch, usize, usize) {
    let unique_start = BASE_QUERY_COUNT + batch_index * UNIQUE_QUERY_COUNT;
    let rows = (0..BASE_QUERY_COUNT)
        .chain(unique_start..unique_start + UNIQUE_QUERY_COUNT)
        .map(|query_index| statement_row(batch_index, query_index))
        .collect::<Vec<_>>();
    let query_bytes = rows
        .iter()
        .filter_map(|row| row.query.as_ref())
        .map(String::len)
        .sum::<usize>();
    let row_count = rows.len();
    let logical_bytes =
        query_bytes.saturating_add(row_count.saturating_mul(STATEMENT_ROW_OVERHEAD_BOUND));
    assert_eq!(row_count, BASE_QUERY_COUNT + UNIQUE_QUERY_COUNT);
    assert!(
        row_count <= BATCH_ROWS,
        "one retained source batch fits the row bound"
    );
    assert_eq!(query_bytes, row_count * QUERY_BYTES);
    assert!(
        logical_bytes <= BATCH_LOGICAL_BYTES,
        "one retained statement batch stays below the source bound"
    );
    (
        PgBatch::Statements(StatementsVersion::V6, rows),
        logical_bytes,
        row_count,
    )
}

fn plan_text(index: usize) -> String {
    let mut plan = query_text(index.saturating_mul(2));
    plan.push_str(&query_text(index.saturating_mul(2).saturating_add(1)));
    assert_eq!(plan.len(), PLAN_BYTES);
    plan
}

fn plan_row(batch_index: usize, plan_index: usize) -> VadvRow {
    let sample = i64::try_from(batch_index).unwrap_or(i64::MAX);
    let identity = i64::try_from(plan_index)
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    VadvRow {
        ts: BASE_TS.saturating_add(sample),
        userid: 16_385,
        dbid: 16_384,
        queryid: identity,
        planid: identity,
        queryid_stat_statements: identity,
        datname: Some("app".to_owned()),
        usename: Some("monitor".to_owned()),
        plan: Some(plan_text(plan_index)),
        calls: sample.saturating_add(1),
        slow_log_calls: 0,
        total_time: 0.0,
        min_time: 0.0,
        max_time: 0.0,
        mean_time: 0.0,
        stddev_time: 0.0,
        rows: identity,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        first_call: BASE_TS,
        last_call: BASE_TS,
        total_plan_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
    }
}

fn plan_batch(batch_index: usize) -> (PgBatch, usize, usize) {
    let first = batch_index.saturating_mul(PLAN_ROWS_PER_BATCH);
    let rows = (first..first + PLAN_ROWS_PER_BATCH)
        .map(|plan_index| plan_row(batch_index, plan_index))
        .collect::<Vec<_>>();
    let logical_bytes = rows
        .len()
        .saturating_mul(PLAN_BYTES.saturating_add(PLAN_ROW_OVERHEAD_BYTES));
    assert!(rows.len() <= BATCH_ROWS);
    assert!(logical_bytes >= BATCH_LOGICAL_BYTES);
    assert!(
        logical_bytes <= BATCH_LOGICAL_BYTES.saturating_add(PLAN_BYTES + PLAN_ROW_OVERHEAD_BYTES),
        "the source checks its byte target after appending a complete row"
    );
    (
        PgBatch::StorePlansVadv(rows),
        logical_bytes,
        PLAN_ROWS_PER_BATCH,
    )
}

impl ReplayAppend<'_> {
    fn append(&mut self, batch: &PgBatch, ts: i64) {
        for attempt in 0..2 {
            let includes_settings = self.segment.needs_pg_settings() && !self.settings.is_empty();
            let opening_settings = if includes_settings {
                self.settings
            } else {
                &[]
            };
            let mut buffers = SectionBuffers::new();
            push_pg_batch(
                &mut buffers,
                self.segment.interner_mut(),
                batch,
                opening_settings,
            )
            .expect("buffer retained PostgreSQL batch");
            let flushed = encode_window(buffers, self.segment.interner())
                .expect("encode retained PostgreSQL batch");
            let appended_bytes = u64::try_from(flushed.summary.part_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(FRAME_HEADER_LEN).unwrap_or(u64::MAX));
            let finished = append_window_and_maybe_close(
                self.journal,
                self.writer,
                self.config,
                self.segment,
                ts,
                false,
                &flushed,
            )
            .expect("append retained PostgreSQL batch");
            let retry = finished.iter().any(|(_, reason)| *reason == "journal-full");
            for (path, reason) in finished {
                self.report.paths.push(path);
                *self.report.close_reasons.entry(reason).or_default() += 1;
            }
            if retry {
                assert_eq!(attempt, 0, "a fresh segment accepts the retained batch");
                continue;
            }
            if includes_settings && !self.segment.is_empty() {
                self.segment.mark_pg_settings_present();
            }
            self.report.cumulative_appended_wal_bytes = self
                .report
                .cumulative_appended_wal_bytes
                .saturating_add(appended_bytes);
            self.report.peak_wal_bytes = self.report.peak_wal_bytes.max(self.journal.bytes());
            self.report.appended_windows += 1;
            return;
        }
        panic!("retained PostgreSQL batch exhausted its append attempts");
    }
}

fn assert_query_rows(
    segment: &kronika_reader::Segment,
    counts: &mut BTreeMap<i64, usize>,
) -> usize {
    let dictionary = segment.dictionary().expect("read segment dictionary");
    let rows = segment
        .rows(PG_STAT_STATEMENTS_V6_TYPE_ID)
        .expect("read statement rows");
    for row in &rows {
        let Some(Cell::I64(query_id)) = row.get("queryid") else {
            panic!("statement queryid must be present");
        };
        let Some(Cell::StrId(query)) = row.get("query") else {
            panic!("statement text must be present");
        };
        let query_index =
            usize::try_from(query_id.saturating_sub(1)).expect("positive queryid fits usize");
        let expected = query_text(query_index);
        match dictionary.resolve(*query) {
            Some(Resolved::Str(actual)) => assert_eq!(actual, expected.as_bytes()),
            Some(Resolved::Blob(_)) => panic!("short statement text belongs in dict.strings"),
            None => panic!("statement text id resolves"),
        }
        *counts.entry(*query_id).or_default() += 1;
    }
    rows.len()
}

fn assert_query_counts(counts: &BTreeMap<i64, usize>) {
    assert_eq!(
        counts.len(),
        BASE_QUERY_COUNT + STATEMENT_BATCH_COUNT * UNIQUE_QUERY_COUNT
    );
    for query_index in 0..BASE_QUERY_COUNT {
        let query_id = i64::try_from(query_index).unwrap_or(i64::MAX) + 1;
        assert_eq!(counts.get(&query_id), Some(&STATEMENT_BATCH_COUNT));
    }
    for query_index in
        BASE_QUERY_COUNT..BASE_QUERY_COUNT + STATEMENT_BATCH_COUNT * UNIQUE_QUERY_COUNT
    {
        let query_id = i64::try_from(query_index).unwrap_or(i64::MAX) + 1;
        assert_eq!(counts.get(&query_id), Some(&1));
    }
}

fn assert_plan_rows(segment: &kronika_reader::Segment, seen: &mut usize) -> usize {
    let dictionary = segment.dictionary().expect("read segment dictionary");
    let rows = segment
        .rows(PG_STORE_PLANS_VADV_TYPE_ID)
        .expect("read plan rows");
    for row in &rows {
        let Some(Cell::I64(plan_id)) = row.get("planid") else {
            panic!("plan id must be present");
        };
        let Some(Cell::StrId(plan)) = row.get("plan") else {
            panic!("plan text must be present");
        };
        let plan_index =
            usize::try_from(plan_id.saturating_sub(1)).expect("positive plan id fits usize");
        let expected = plan_text(plan_index);
        match dictionary.resolve(*plan) {
            Some(Resolved::Blob(actual)) => {
                assert_eq!(actual.stored_bytes, expected.as_bytes());
                assert_eq!(
                    actual.full_len,
                    u64::try_from(PLAN_BYTES).unwrap_or(u64::MAX)
                );
                assert!(!actual.truncated);
                assert!(actual.full_sha256.is_none());
            }
            Some(Resolved::Str(_)) => panic!("plan text belongs in dict.blobs"),
            None => panic!("plan text id resolves"),
        }
        *seen += 1;
    }
    rows.len()
}

#[test]
fn statement_sql_timestamp_survives_source_batches_in_one_active_segment() {
    let directory = tempfile::tempdir().expect("create statement timestamp directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open statement timestamp journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let mut scheduler = Scheduler::new(Intervals::default());
    let mut process_io = ProcessIoCredentials::new();
    let mut rows = (0..=BATCH_ROWS)
        .map(|query_index| {
            let mut row = statement_row(0, query_index);
            row.query = Some(format!("select {query_index}"));
            row
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), BATCH_ROWS + 1);
    assert!(rows.len() > 256);
    let last = rows.pop().expect("row beyond the source batch bound");
    let first_batch = PgBatch::Statements(StatementsVersion::V6, rows);
    append_pending_pg_batch(
        &mut journal,
        &writer,
        &config,
        false,
        &first_batch,
        &[],
        BASE_TS + 10,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    )
    .expect("append the first natural SQL timestamp batch");

    let reader = Reader::open(directory.path()).expect("open active statement prefix reader");
    let listing = reader.segments(..).expect("list active statement prefix");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(listing.segments[0].kind(), SegmentKind::Active);
    let active = reader
        .open_segment(&listing.segments[0])
        .expect("open active statement prefix");
    let active_rows = active
        .rows(PG_STAT_STATEMENTS_V6_TYPE_ID)
        .expect("read the first statement source batch");
    assert_eq!(active_rows.len(), BATCH_ROWS);
    assert!(
        active_rows
            .iter()
            .all(|row| row.get("ts") == Some(&Cell::Ts(BASE_TS)))
    );

    let second_batch = PgBatch::Statements(StatementsVersion::V6, vec![last]);
    let outcome = append_pending_pg_batch(
        &mut journal,
        &writer,
        &config,
        false,
        &second_batch,
        &[],
        BASE_TS + 20,
        &mut process_io,
        &mut segment,
        &mut scheduler,
    )
    .expect("append the remaining natural SQL timestamp row");
    assert!(outcome.written.is_empty());

    let reader = Reader::open(directory.path()).expect("open statement reader");
    let listing = reader.segments(..).expect("list statement segments");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(listing.segments[0].kind(), SegmentKind::Active);
    let mut query_ids = Vec::new();
    let mut segment_rows = Vec::new();
    for reference in &listing.segments {
        let stored = reader
            .open_segment(reference)
            .expect("open statement timestamp segment");
        let rows = stored
            .rows(PG_STAT_STATEMENTS_V6_TYPE_ID)
            .expect("read statement timestamp rows");
        segment_rows.push(rows.len());
        for row in rows {
            assert_eq!(row.get("ts"), Some(&Cell::Ts(BASE_TS)));
            let Some(Cell::I64(query_id)) = row.get("queryid") else {
                panic!("statement queryid is present");
            };
            query_ids.push(*query_id);
        }
    }
    assert_eq!(segment_rows, [BATCH_ROWS + 1]);
    query_ids.sort_unstable();
    assert_eq!(
        query_ids,
        (1..=i64::try_from(BATCH_ROWS + 1).expect("statement row count fits i64"))
            .collect::<Vec<_>>()
    );
}

fn read_replay_artifacts(root: &Path, expected_paths: usize) -> ReplayArtifactReport {
    let reader = Reader::open(root).expect("open replay reader");
    let listing = reader.segments(..).expect("list replay segments");
    assert!(listing.warnings.is_empty());
    assert_eq!(listing.segments.len(), expected_paths);

    let mut report = ReplayArtifactReport {
        segments: listing.segments.len(),
        zms_bytes: 0,
        section_body_bytes: 0,
        overhead_bytes: 0,
        windows: 0,
        statement_rows: 0,
        plan_rows: 0,
        rss_kib: 0,
        sections: BTreeMap::new(),
    };
    let mut query_counts = BTreeMap::new();
    let mut plans_seen = 0;
    for reference in &listing.segments {
        assert_eq!(reference.kind(), SegmentKind::Finished);
        let segment = reader
            .open_segment(reference)
            .expect("open finished replay segment");
        report.zms_bytes = report.zms_bytes.saturating_add(segment.captured_bytes());
        report.windows = report
            .windows
            .saturating_add(u64::from(segment.window_count()));
        for (type_id, section) in segment.sections() {
            report.section_body_bytes = report.section_body_bytes.saturating_add(section.bytes);
            let aggregate = report
                .sections
                .entry(section_name(type_id).unwrap_or("unknown"))
                .or_default();
            aggregate.rows = aggregate.rows.saturating_add(section.rows);
            aggregate.bytes = aggregate.bytes.saturating_add(section.bytes);
        }
        report.statement_rows = report
            .statement_rows
            .saturating_add(assert_query_rows(&segment, &mut query_counts));
        report.plan_rows = report
            .plan_rows
            .saturating_add(assert_plan_rows(&segment, &mut plans_seen));
    }
    assert_query_counts(&query_counts);
    assert_eq!(plans_seen, PLAN_BATCH_COUNT * PLAN_ROWS_PER_BATCH);
    report.overhead_bytes = report
        .zms_bytes
        .checked_sub(report.section_body_bytes)
        .expect("section bodies fit inside ZMS files");
    report.rss_kib = peak_rss_kib();
    report
}

fn assert_replay_costs(
    write: &ReplayWriteReport,
    artifact: &ReplayArtifactReport,
    max_journal_len: usize,
) {
    assert_eq!(artifact.segments, 1);
    assert_eq!(
        artifact.windows,
        u64::try_from(STATEMENT_BATCH_COUNT + PLAN_BATCH_COUNT).unwrap_or(u64::MAX)
    );
    assert_eq!(
        artifact.statement_rows,
        STATEMENT_BATCH_COUNT * (BASE_QUERY_COUNT + UNIQUE_QUERY_COUNT)
    );
    assert_eq!(artifact.plan_rows, PLAN_BATCH_COUNT * PLAN_ROWS_PER_BATCH);
    assert_eq!(write.close_reasons.get("test-end"), Some(&1));
    assert!(write.peak_wal_bytes <= max_journal_len);
    assert!(artifact.zms_bytes <= MAX_REPLAY_ZMS_BYTES);
    assert!(artifact.zms_bytes.saturating_mul(10) <= PRE_CHANGE_ZMS_BYTES.saturating_mul(9));
    assert!(write.cumulative_appended_wal_bytes <= MAX_REPLAY_WAL_BYTES);
    let strings = artifact
        .sections
        .get("dict.strings")
        .expect("replay carries its strings dictionary");
    assert!(strings.bytes <= MAX_REPLAY_DICT_STRINGS_BYTES);
    assert!(strings.bytes.saturating_mul(5) <= PRE_CHANGE_DICT_STRINGS_BYTES.saturating_mul(4));
    let blobs = artifact
        .sections
        .get("dict.blobs")
        .expect("replay carries its blobs dictionary");
    assert!(blobs.bytes <= MAX_REPLAY_DICT_BLOBS_BYTES);
}

fn print_replay_costs(write: ReplayWriteReport, artifact: ReplayArtifactReport) {
    println!(
        "zms_replay segments={} zms_bytes={} peak_wal_bytes={} cumulative_appended_wal_bytes={} section_body_bytes={} overhead_bytes={} windows={} statement_rows={} plan_rows={} max_batch_rows={} max_batch_logical_bytes={} peak_rss_kib={}",
        artifact.segments,
        artifact.zms_bytes,
        write.peak_wal_bytes,
        write.cumulative_appended_wal_bytes,
        artifact.section_body_bytes,
        artifact.overhead_bytes,
        artifact.windows,
        artifact.statement_rows,
        artifact.plan_rows,
        write.max_batch_rows,
        write.max_batch_logical_bytes,
        artifact.rss_kib,
    );
    for (reason, count) in write.close_reasons {
        println!("zms_replay_close reason={reason} count={count}");
    }
    for (name, section) in artifact.sections {
        println!(
            "zms_replay_section name={name} rows={} bytes={}",
            section.rows, section.bytes
        );
    }
}

#[test]
fn bounded_postgres_batches_report_finished_segment_costs() {
    let directory = tempfile::tempdir().expect("create replay directory");
    let writer = owner(directory.path());
    let journal_config = JournalConfig::default();
    let max_journal_len = journal_config.max_journal_len;
    let mut journal = Journal::open(&writer, journal_config).expect("open replay journal");
    let config = config(
        directory.path(),
        u64::try_from(max_journal_len).expect("journal bound fits u64"),
    );
    let settings = [settings_row()];
    let mut segment = SegmentState::default();
    let mut write_report = ReplayWriteReport::default();

    {
        let mut replay = ReplayAppend {
            journal: &mut journal,
            writer: &writer,
            config: &config,
            segment: &mut segment,
            settings: &settings,
            report: &mut write_report,
        };
        for batch_index in 0..STATEMENT_BATCH_COUNT {
            let (batch, logical_bytes, batch_rows) = statement_batch(batch_index);
            replay.report.max_batch_rows = replay.report.max_batch_rows.max(batch_rows);
            replay.report.max_batch_logical_bytes =
                replay.report.max_batch_logical_bytes.max(logical_bytes);
            replay.append(
                &batch,
                BASE_TS + i64::try_from(batch_index).unwrap_or(i64::MAX),
            );
        }
        for batch_index in 0..PLAN_BATCH_COUNT {
            let (batch, logical_bytes, batch_rows) = plan_batch(batch_index);
            replay.report.max_batch_rows = replay.report.max_batch_rows.max(batch_rows);
            replay.report.max_batch_logical_bytes =
                replay.report.max_batch_logical_bytes.max(logical_bytes);
            replay.append(
                &batch,
                BASE_TS + i64::try_from(STATEMENT_BATCH_COUNT + batch_index).unwrap_or(i64::MAX),
            );
        }
    }

    assert_eq!(
        write_report.appended_windows,
        STATEMENT_BATCH_COUNT + PLAN_BATCH_COUNT
    );
    assert!(
        write_report.paths.is_empty(),
        "bounded PostgreSQL batches stay in the open logical segment"
    );
    assert!(
        write_report.close_reasons.is_empty(),
        "no retained batch requests an early write"
    );
    assert!(write_report.peak_wal_bytes <= max_journal_len);
    if !segment.is_empty() {
        let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
            .expect("write final replay segment");
        write_report.paths.push(path);
        *write_report.close_reasons.entry("test-end").or_default() += 1;
    }
    let artifact = read_replay_artifacts(directory.path(), write_report.paths.len());
    assert_eq!(
        write_report
            .paths
            .iter()
            .map(|path| std::fs::metadata(path).expect("stat replay segment").len())
            .sum::<u64>(),
        artifact.zms_bytes
    );
    assert_replay_costs(&write_report, &artifact, max_journal_len);
    print_replay_costs(write_report, artifact);
}

#[test]
fn wal_storage_hour_reports_raw_and_finished_costs() {
    let directory = tempfile::tempdir().expect("create WAL storage cost directory");
    let writer = owner(directory.path());
    let journal_config = JournalConfig::default();
    let mut journal = Journal::open(&writer, journal_config).expect("open WAL storage journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let mut write_report = ReplayWriteReport::default();

    {
        let mut replay = ReplayAppend {
            journal: &mut journal,
            writer: &writer,
            config: &config,
            segment: &mut segment,
            settings: &[],
            report: &mut write_report,
        };
        for sample in 0..WAL_STORAGE_SNAPSHOTS_PER_HOUR {
            let sample = i64::try_from(sample).expect("sample count fits i64");
            let ts = BASE_TS.saturating_add(sample.saturating_mul(30_000_000));
            replay.append(
                &PgBatch::WalStorage(PgWalStorage {
                    ts: Ts(ts),
                    wal_files_bytes: 16_777_216_i64.saturating_mul(sample.saturating_add(1)),
                }),
                ts,
            );
        }
    }

    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write WAL storage cost segment");
    let reader = Reader::open(directory.path()).expect("open WAL storage cost reader");
    let listing = reader.segments(..).expect("list WAL storage cost segment");
    let reference = listing.segments.first().expect("one WAL storage segment");
    let stored = reader
        .open_segment(reference)
        .expect("open finished WAL storage segment");
    let rows = stored
        .rows(PG_WAL_STORAGE_TYPE_ID)
        .expect("read WAL storage rows");
    let section = stored
        .sections()
        .find(|(type_id, _section)| *type_id == PG_WAL_STORAGE_TYPE_ID)
        .map(|(_type_id, section)| section)
        .expect("WAL storage section is catalogued");
    let zms_bytes = std::fs::metadata(path)
        .expect("stat WAL storage segment")
        .len();
    let marginal_zms_bytes = section
        .bytes
        .saturating_add(u64::try_from(ENTRY_LEN).expect("catalog entry length fits u64"));

    assert_eq!(listing.segments.len(), 1);
    assert_eq!(
        write_report.appended_windows,
        WAL_STORAGE_SNAPSHOTS_PER_HOUR
    );
    assert_eq!(
        stored.window_count(),
        u32::try_from(WAL_STORAGE_SNAPSHOTS_PER_HOUR).expect("sample count fits u32")
    );
    assert_eq!(rows.len(), WAL_STORAGE_SNAPSHOTS_PER_HOUR);
    assert_eq!(
        rows.first().and_then(|row| row.get("wal_files_bytes")),
        Some(&Cell::I64(16_777_216))
    );
    assert_eq!(
        rows.last().and_then(|row| row.get("wal_files_bytes")),
        Some(&Cell::I64(2_013_265_920))
    );
    assert!(raw_wal_bytes < 256 * 1024);
    assert!(section.bytes < 4 * 1024);
    assert!(zms_bytes < 8 * 1024);
    println!(
        "pg_wal_storage_cost rows={} raw_wal_bytes={} section_bytes={} marginal_zms_bytes={} zms_bytes={}",
        rows.len(),
        raw_wal_bytes,
        section.bytes,
        marginal_zms_bytes,
        zms_bytes
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the production cost artifact keeps its complete write, readback, and measurement path together"
)]
fn activity_datid_hour_reports_production_writer_costs() {
    let directory = tempfile::tempdir().expect("create Activity cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open Activity journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let mut write_report = ReplayWriteReport::default();
    let cpu_before = self_cpu_ticks();
    let started = std::time::Instant::now();

    {
        let mut replay = ReplayAppend {
            journal: &mut journal,
            writer: &writer,
            config: &config,
            segment: &mut segment,
            settings: &[],
            report: &mut write_report,
        };
        for sample in 0..ACTIVITY_SNAPSHOTS_PER_HOUR {
            let sample = i64::try_from(sample).expect("sample count fits i64");
            let ts = BASE_TS.saturating_add(sample.saturating_mul(30_000_000));
            let rows = (0..ACTIVITY_ROWS_PER_SNAPSHOT)
                .map(|backend| {
                    let backend = i32::try_from(backend).expect("backend count fits i32");
                    let background = backend % 16 == 15;
                    ActivityRow {
                        ts,
                        pid: 10_000_i32.saturating_add(backend),
                        leader_pid: None,
                        datid: (!background).then_some(
                            16_384_u32
                                .saturating_add(u32::try_from(backend % 4).unwrap_or_default()),
                        ),
                        datname: (!background).then(|| format!("app_{}", backend % 4)),
                        usename: (!background).then(|| "application".to_owned()),
                        application_name: if background {
                            String::new()
                        } else {
                            "postgres-driver".to_owned()
                        },
                        client_addr: if background {
                            String::new()
                        } else {
                            "10.0.0.10".to_owned()
                        },
                        backend_type: if background {
                            "autovacuum worker".to_owned()
                        } else {
                            "client backend".to_owned()
                        },
                        state: (!background).then(|| "active".to_owned()),
                        wait_event_type: (backend % 5 == 0).then(|| "Lock".to_owned()),
                        wait_event: (backend % 5 == 0).then(|| "transactionid".to_owned()),
                        query: (!background)
                            .then(|| format!("select payload from work_{}", backend % 16)),
                        query_id: (!background)
                            .then_some(90_000_i64.saturating_add(i64::from(backend % 16))),
                        backend_xid_age: (!background)
                            .then_some(i64::from(backend).saturating_mul(10)),
                        backend_xmin_age: (!background)
                            .then_some(i64::from(backend).saturating_mul(20)),
                        backend_start: BASE_TS
                            .saturating_sub(i64::from(backend).saturating_mul(1_000_000)),
                        xact_start: (!background).then_some(ts.saturating_sub(5_000_000)),
                        query_start: (!background).then_some(ts.saturating_sub(2_000_000)),
                        state_change: (!background).then_some(ts.saturating_sub(2_000_000)),
                    }
                })
                .collect();
            replay.append(&PgBatch::Activity(ActivityVersion::V3, rows), ts);
        }
    }

    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write Activity cost segment");
    let reader = Reader::open(directory.path()).expect("open Activity cost reader");
    let listing = reader.segments(..).expect("list Activity cost segment");
    let reference = listing.segments.first().expect("one Activity segment");
    let stored = reader
        .open_segment(reference)
        .expect("open finished Activity segment");
    let rows = stored
        .rows(PG_STAT_ACTIVITY_V3_TYPE_ID)
        .expect("read Activity rows");
    let section = stored
        .sections()
        .find(|(type_id, _section)| *type_id == PG_STAT_ACTIVITY_V3_TYPE_ID)
        .map(|(_type_id, section)| section)
        .expect("Activity section is catalogued");
    let zms_bytes = std::fs::metadata(path)
        .expect("stat Activity segment")
        .len();
    let marginal_zms_bytes = section
        .bytes
        .saturating_add(u64::try_from(ENTRY_LEN).expect("catalog entry length fits u64"));
    let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let cpu_ticks = self_cpu_ticks().saturating_sub(cpu_before);
    let rss_kib = peak_rss_kib();

    assert_eq!(listing.segments.len(), 1);
    assert_eq!(write_report.appended_windows, ACTIVITY_SNAPSHOTS_PER_HOUR);
    assert_eq!(
        stored.window_count(),
        u32::try_from(ACTIVITY_SNAPSHOTS_PER_HOUR).expect("sample count fits u32")
    );
    assert_eq!(
        rows.len(),
        ACTIVITY_SNAPSHOTS_PER_HOUR.saturating_mul(ACTIVITY_ROWS_PER_SNAPSHOT)
    );
    assert!(
        rows.iter()
            .any(|row| matches!(row.get("datid"), Some(Cell::U32(16_384..=16_387))))
    );
    assert!(rows.iter().any(|row| row.get("datid") == Some(&Cell::Null)));
    assert!(raw_wal_bytes < 16 * 1024 * 1024);
    assert!(section.bytes < 2 * 1024 * 1024);
    assert!(zms_bytes < 2 * 1024 * 1024);
    println!(
        "pg_stat_activity_datid_cost rows={} raw_wal_bytes={} peak_wal_bytes={} section_bytes={} marginal_zms_bytes={} zms_bytes={} elapsed_us={} cpu_ticks={} peak_rss_kib={}",
        rows.len(),
        raw_wal_bytes,
        write_report.peak_wal_bytes,
        section.bytes,
        marginal_zms_bytes,
        zms_bytes,
        elapsed_us,
        cpu_ticks,
        rss_kib
    );
}

#[derive(Clone, Copy)]
struct RelationCostSpec {
    name: &'static str,
    tables: usize,
    indexes: usize,
    snapshots: usize,
    high_cardinality: bool,
    all_layouts: bool,
    max_raw_wal_bytes: u64,
    max_peak_wal_bytes: u64,
    max_zms_bytes: u64,
    max_cpu_ticks: u64,
    max_peak_rss_kib: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct RelationCostReport {
    raw_wal_bytes: u64,
    peak_wal_bytes: u64,
    zms_bytes: u64,
    segments: usize,
    frames: usize,
    parts: usize,
    rows: BTreeMap<u32, u64>,
    raw_section_bytes: BTreeMap<u32, u64>,
    zms_section_bytes: BTreeMap<u32, u64>,
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the relation format acceptance artifact keeps all six layouts and production WAL/ZMS accounting together"
)]
fn relation_tablespace_layouts_report_production_writer_costs() {
    let profiles = [
        RelationCostSpec {
            name: "layout_matrix",
            tables: RELATION_REPRESENTATIVE_TABLES,
            indexes: RELATION_REPRESENTATIVE_INDEXES,
            snapshots: 1,
            high_cardinality: false,
            all_layouts: true,
            max_raw_wal_bytes: 700_000,
            max_peak_wal_bytes: 700_000,
            max_zms_bytes: 400_000,
            max_cpu_ticks: 150,
            max_peak_rss_kib: 64 * 1_024,
        },
        RelationCostSpec {
            name: "representative_shared",
            tables: RELATION_REPRESENTATIVE_TABLES,
            indexes: RELATION_REPRESENTATIVE_INDEXES,
            snapshots: RELATION_SNAPSHOTS,
            high_cardinality: false,
            all_layouts: false,
            max_raw_wal_bytes: 2_500_000,
            max_peak_wal_bytes: 2_500_000,
            max_zms_bytes: 300_000,
            max_cpu_ticks: 500,
            max_peak_rss_kib: 72 * 1_024,
        },
        RelationCostSpec {
            name: "stress_shared",
            tables: RELATION_STRESS_OBJECTS,
            indexes: RELATION_STRESS_OBJECTS,
            snapshots: RELATION_SNAPSHOTS,
            high_cardinality: false,
            all_layouts: false,
            max_raw_wal_bytes: 9_000_000,
            max_peak_wal_bytes: 9_000_000,
            max_zms_bytes: 2_800_000,
            max_cpu_ticks: 1_800,
            max_peak_rss_kib: 96 * 1_024,
        },
        RelationCostSpec {
            name: "stress_high_cardinality",
            tables: RELATION_STRESS_OBJECTS,
            indexes: RELATION_STRESS_OBJECTS,
            snapshots: 2,
            high_cardinality: true,
            all_layouts: false,
            max_raw_wal_bytes: 2_200_000,
            max_peak_wal_bytes: 2_200_000,
            max_zms_bytes: 900_000,
            max_cpu_ticks: 350,
            max_peak_rss_kib: 96 * 1_024,
        },
    ];
    let Some(profile) = std::env::var_os(RELATION_COST_PROFILE_ENV) else {
        let executable = std::env::current_exe().expect("locate collector test binary");
        for spec in profiles {
            let output = std::process::Command::new(&executable)
                .args([
                    "--exact",
                    RELATION_COST_TEST,
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(RELATION_COST_PROFILE_ENV, spec.name)
                .output()
                .expect("run isolated relation cost child");
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            assert!(
                output.status.success(),
                "isolated relation cost child for {} exited with {}",
                spec.name,
                output.status
            );
        }
        return;
    };
    let profile = profile.to_string_lossy();
    let spec = profiles
        .into_iter()
        .find(|spec| spec.name == profile)
        .unwrap_or_else(|| panic!("unknown relation cost profile {profile}"));
    let mut byte_baseline = None;
    for repeat in 0..3 {
        let cpu_before = self_cpu_ticks();
        let started = std::time::Instant::now();
        let report = relation_cost_profile(spec);
        let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let cpu_ticks = self_cpu_ticks().saturating_sub(cpu_before);
        let rss_kib = peak_rss_kib();
        let bytes = (
            report.raw_wal_bytes,
            report.zms_bytes,
            report.raw_section_bytes.clone(),
            report.zms_section_bytes.clone(),
        );
        if let Some(expected) = &byte_baseline {
            assert_eq!(&bytes, expected, "production bytes are deterministic");
        } else {
            byte_baseline = Some(bytes);
        }
        println!(
            "pg_relation_tablespace_cost profile={} repeat={} table_rows={} index_rows={} raw_wal_bytes={} peak_wal_bytes={} zms_bytes={} segments={} frames={} parts={} raw_sections={:?} zms_sections={:?} rows={:?} elapsed_us={} cpu_ticks={} peak_rss_kib={}",
            spec.name,
            repeat + 1,
            spec.tables.saturating_mul(spec.snapshots),
            spec.indexes.saturating_mul(spec.snapshots),
            report.raw_wal_bytes,
            report.peak_wal_bytes,
            report.zms_bytes,
            report.segments,
            report.frames,
            report.parts,
            report.raw_section_bytes,
            report.zms_section_bytes,
            report.rows,
            elapsed_us,
            cpu_ticks,
            rss_kib,
        );
        assert_eq!(report.frames, report.parts);
        assert!(report.segments > 0);
        assert!(report.raw_wal_bytes >= report.peak_wal_bytes);
        assert!(report.zms_bytes > 0);
        if spec.name == "stress_shared" {
            assert_eq!(report.segments, 1);
            assert_eq!(report.peak_wal_bytes, report.raw_wal_bytes);
            assert!(report.zms_bytes.saturating_mul(8) < report.raw_wal_bytes);
        }
        assert!(
            report.raw_wal_bytes <= spec.max_raw_wal_bytes,
            "{} raw WAL bytes {} exceed {}",
            spec.name,
            report.raw_wal_bytes,
            spec.max_raw_wal_bytes
        );
        assert!(
            report.peak_wal_bytes <= spec.max_peak_wal_bytes,
            "{} peak WAL bytes {} exceed {}",
            spec.name,
            report.peak_wal_bytes,
            spec.max_peak_wal_bytes
        );
        assert!(
            report.zms_bytes <= spec.max_zms_bytes,
            "{} ZMS bytes {} exceed {}",
            spec.name,
            report.zms_bytes,
            spec.max_zms_bytes
        );
        assert!(
            cpu_ticks <= spec.max_cpu_ticks,
            "{} CPU ticks {} exceed {}",
            spec.name,
            cpu_ticks,
            spec.max_cpu_ticks
        );
        assert!(
            rss_kib <= spec.max_peak_rss_kib,
            "{} peak RSS KiB {} exceed {}",
            spec.name,
            rss_kib,
            spec.max_peak_rss_kib
        );
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the production writer report keeps its byte accounting and reader validation adjacent"
)]
fn relation_cost_profile(spec: RelationCostSpec) -> RelationCostReport {
    let artifact_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-artifacts");
    std::fs::create_dir_all(&artifact_root).expect("create relation cost artifact root");
    let directory = tempfile::Builder::new()
        .prefix("relation-cost-")
        .tempdir_in(&artifact_root)
        .expect("create relation cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open relation cost journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let table_versions: &[UserTablesVersion] = if spec.all_layouts {
        &[
            UserTablesVersion::V1,
            UserTablesVersion::V2,
            UserTablesVersion::V3,
            UserTablesVersion::V4,
        ]
    } else {
        &[UserTablesVersion::V4]
    };
    let index_versions: &[UserIndexesVersion] = if spec.all_layouts {
        &[UserIndexesVersion::V1, UserIndexesVersion::V2]
    } else {
        &[UserIndexesVersion::V2]
    };
    let mut raw_section_bytes = BTreeMap::<u32, u64>::new();
    let mut frames = 0_usize;
    let mut parts = 0_usize;
    let mut appended_wal_bytes = 0_u64;
    let mut paths = Vec::new();
    let mut peak_wal_bytes = journal.bytes();
    for sample in 0..spec.snapshots {
        let ts = BASE_TS.saturating_add(
            i64::try_from(sample)
                .unwrap_or(i64::MAX)
                .saturating_mul(30_000_000),
        );
        for &version in table_versions {
            let rows = (0..spec.tables)
                .map(|object| relation_table_row(ts, sample, object, spec.high_cardinality))
                .collect();
            paths.extend(append_relation_cost_batch(
                &mut journal,
                &writer,
                &config,
                &mut segment,
                &PgBatch::UserTables(version, rows),
                ts,
                &mut raw_section_bytes,
                &mut frames,
                &mut parts,
                &mut appended_wal_bytes,
            ));
            peak_wal_bytes = peak_wal_bytes.max(journal.bytes());
        }
        for &version in index_versions {
            let rows = (0..spec.indexes)
                .map(|object| relation_index_row(ts, sample, object, spec.high_cardinality))
                .collect();
            paths.extend(append_relation_cost_batch(
                &mut journal,
                &writer,
                &config,
                &mut segment,
                &PgBatch::UserIndexes(version, rows),
                ts,
                &mut raw_section_bytes,
                &mut frames,
                &mut parts,
                &mut appended_wal_bytes,
            ));
            peak_wal_bytes = peak_wal_bytes.max(journal.bytes());
        }
    }
    paths.push(
        close_open_segment(&mut journal, &writer, &mut segment, "test-end")
            .expect("write relation cost segment"),
    );
    let raw_wal_bytes = appended_wal_bytes.saturating_add(
        u64::try_from(paths.len().saturating_mul(JOURNAL_HEADER_LEN)).unwrap_or(u64::MAX),
    );
    let reader = Reader::open(directory.path()).expect("open relation cost reader");
    let listing = reader.segments(..).expect("list relation cost segment");
    assert_eq!(listing.segments.len(), paths.len());
    let mut rows = BTreeMap::<u32, u64>::new();
    let mut zms_section_bytes = BTreeMap::<u32, u64>::new();
    for reference in &listing.segments {
        let stored = reader
            .open_segment(reference)
            .expect("open relation cost segment");
        for (type_id, section) in stored.sections() {
            if matches!(type_id, 1_013_005..=1_013_008 | 1_014_003..=1_014_004) {
                let stored_rows = rows.entry(type_id).or_default();
                *stored_rows = stored_rows.saturating_add(section.rows);
                let stored_bytes = zms_section_bytes.entry(type_id).or_default();
                *stored_bytes = stored_bytes.saturating_add(section.bytes);
            }
        }
    }
    let zms_bytes = paths.iter().fold(0_u64, |total, path| {
        total.saturating_add(
            std::fs::metadata(path)
                .expect("stat relation cost segment")
                .len(),
        )
    });
    RelationCostReport {
        raw_wal_bytes,
        peak_wal_bytes: u64::try_from(peak_wal_bytes).unwrap_or(u64::MAX),
        zms_bytes,
        segments: paths.len(),
        frames,
        parts,
        rows,
        raw_section_bytes,
        zms_section_bytes,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the production append accounting is explicit"
)]
fn append_relation_cost_batch(
    journal: &mut Journal,
    writer: &WriterOwner,
    config: &Config,
    segment: &mut SegmentState,
    batch: &PgBatch,
    ts: i64,
    raw_section_bytes: &mut BTreeMap<u32, u64>,
    frames: &mut usize,
    parts: &mut usize,
    appended_wal_bytes: &mut u64,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for attempt in 0..2 {
        let mut buffers = SectionBuffers::new();
        push_pg_batch(&mut buffers, segment.interner_mut(), batch, &[])
            .expect("buffer relation cost rows");
        let flushed =
            encode_window(buffers, segment.interner()).expect("encode relation cost window");
        let completed =
            append_window_and_maybe_close(journal, writer, config, segment, ts, false, &flushed)
                .expect("append relation cost window");
        let retry = completed
            .iter()
            .any(|(_, reason)| *reason == "journal-full");
        paths.extend(completed.into_iter().map(|(path, _reason)| path));
        if retry {
            assert_eq!(attempt, 0, "a fresh segment accepts the relation batch");
            continue;
        }
        for section in &flushed.summary.sections {
            if matches!(
                section.type_id,
                1_013_005..=1_013_008 | 1_014_003..=1_014_004
            ) {
                let total = raw_section_bytes.entry(section.type_id).or_default();
                *total =
                    total.saturating_add(u64::try_from(section.body_bytes).unwrap_or(u64::MAX));
            }
        }
        *appended_wal_bytes = appended_wal_bytes
            .saturating_add(u64::try_from(flushed.summary.part_bytes).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(FRAME_HEADER_LEN).unwrap_or(u64::MAX));
        *frames = frames.saturating_add(1);
        *parts = parts.saturating_add(1);
        return paths;
    }
    unreachable!("the relation batch is appended within two attempts")
}

fn relation_table_row(
    ts: i64,
    sample: usize,
    object: usize,
    high_cardinality: bool,
) -> UserTablesRow {
    let object_i64 = i64::try_from(object).unwrap_or(i64::MAX);
    let counter = i64::try_from(sample)
        .unwrap_or(i64::MAX)
        .saturating_mul(100)
        .saturating_add(object_i64);
    let parent = object.is_multiple_of(97);
    let has_index = !object.is_multiple_of(7);
    let has_toast = !parent && object.is_multiple_of(4);
    let (tablespace_oid, tablespace) = if parent {
        (None, None)
    } else if high_cardinality {
        (
            Some(20_000_u32.saturating_add(u32::try_from(object).unwrap_or(u32::MAX))),
            Some(format!("tenant_ts_{object:05}")),
        )
    } else {
        match object % 10 {
            0 => (Some(9_000), Some("db_default_ts".to_owned())),
            1 => (Some(9_001), Some("heap_ts".to_owned())),
            _ => (Some(1_663), Some("pg_default".to_owned())),
        }
    };
    UserTablesRow {
        ts,
        datid: 16_384_u32.saturating_add(u32::try_from(object % 8).unwrap_or_default()),
        datname: format!("app_{}", object % 8).into(),
        relid: 100_000_u32.saturating_add(u32::try_from(object).unwrap_or(u32::MAX)),
        schemaname: format!("schema_{}", object % 32),
        relname: format!("table_{object:05}"),
        tablespace_oid,
        tablespace,
        seq_scan: counter,
        seq_tup_read: counter.saturating_mul(10),
        idx_scan: has_index.then_some(counter.saturating_mul(2)),
        idx_tup_fetch: has_index.then_some(counter.saturating_mul(5)),
        n_tup_ins: counter,
        n_tup_upd: counter / 2,
        n_tup_del: counter / 5,
        n_tup_hot_upd: counter / 4,
        n_tup_newpage_upd: Some(counter / 8),
        n_live_tup: object_i64.saturating_mul(1_000),
        n_dead_tup: object_i64.saturating_mul(10),
        n_mod_since_analyze: counter,
        n_ins_since_vacuum: Some(counter),
        vacuum_count: i64::try_from(sample).unwrap_or(i64::MAX),
        autovacuum_count: i64::try_from(sample / 2).unwrap_or(i64::MAX),
        analyze_count: i64::try_from(sample).unwrap_or(i64::MAX),
        autoanalyze_count: i64::try_from(sample / 2).unwrap_or(i64::MAX),
        last_vacuum: (sample > 0).then_some(ts.saturating_sub(1_000_000)),
        last_autovacuum: (sample > 1).then_some(ts.saturating_sub(2_000_000)),
        last_analyze: (sample > 0).then_some(ts.saturating_sub(500_000)),
        last_autoanalyze: (sample > 1).then_some(ts.saturating_sub(750_000)),
        last_seq_scan: (sample > 0).then_some(ts.saturating_sub(250_000)),
        last_idx_scan: (sample > 0 && has_index).then_some(ts.saturating_sub(125_000)),
        total_vacuum_time: Some(f64::from(u32::try_from(sample).unwrap_or(u32::MAX))),
        total_autovacuum_time: Some(f64::from(
            u32::try_from(sample.saturating_mul(2)).unwrap_or(u32::MAX),
        )),
        total_analyze_time: Some(f64::from(
            u32::try_from(sample.saturating_mul(3)).unwrap_or(u32::MAX),
        )),
        total_autoanalyze_time: Some(f64::from(
            u32::try_from(sample.saturating_mul(4)).unwrap_or(u32::MAX),
        )),
        main_fork_bytes: if parent {
            0
        } else {
            object_i64.saturating_add(1).saturating_mul(8_192)
        },
        toast_bytes: has_toast.then_some(16_384),
        toast_n_live_tup: has_toast.then_some(100),
        toast_n_dead_tup: has_toast.then_some(5),
        toast_last_autovacuum: (has_toast && sample > 0).then_some(ts.saturating_sub(3_000_000)),
        xid_age: (!parent).then_some(object_i64.saturating_mul(10)),
        mxid_age: (!parent).then_some(object_i64),
        reltuples: if parent {
            -1
        } else {
            object_i64.saturating_mul(1_000)
        },
        heap_blks_read: counter,
        heap_blks_hit: counter.saturating_mul(9),
        idx_blks_read: has_index.then_some(counter / 2),
        idx_blks_hit: has_index.then_some(counter.saturating_mul(4)),
        toast_blks_read: has_toast.then_some(counter / 10),
        toast_blks_hit: has_toast.then_some(counter),
        tidx_blks_read: has_toast.then_some(counter / 20),
        tidx_blks_hit: has_toast.then_some(counter / 2),
    }
}

fn relation_index_row(
    ts: i64,
    sample: usize,
    object: usize,
    high_cardinality: bool,
) -> UserIndexesRow {
    let object_i64 = i64::try_from(object).unwrap_or(i64::MAX);
    let counter = i64::try_from(sample)
        .unwrap_or(i64::MAX)
        .saturating_mul(100)
        .saturating_add(object_i64);
    let (tablespace_oid, tablespace) = if high_cardinality {
        (
            30_000_u32.saturating_add(u32::try_from(object).unwrap_or(u32::MAX)),
            Some(format!("index_ts_{object:05}")),
        )
    } else if object % 10 == 1 {
        (9_002, Some("index_ts".to_owned()))
    } else {
        (1_663, Some("pg_default".to_owned()))
    };
    UserIndexesRow {
        ts,
        datid: 16_384_u32.saturating_add(u32::try_from(object % 8).unwrap_or_default()),
        datname: format!("app_{}", object % 8).into(),
        indexrelid: 200_000_u32.saturating_add(u32::try_from(object).unwrap_or(u32::MAX)),
        relid: 100_000_u32.saturating_add(u32::try_from(object / 2).unwrap_or(u32::MAX)),
        schemaname: format!("schema_{}", object % 32),
        relname: format!("table_{:05}", object / 2),
        indexrelname: format!("index_{object:05}"),
        tablespace_oid,
        tablespace,
        idx_scan: counter,
        idx_tup_read: counter.saturating_mul(4),
        idx_tup_fetch: counter.saturating_mul(2),
        main_fork_bytes: object_i64.saturating_add(1).saturating_mul(4_096),
        last_idx_scan: (sample > 0).then_some(ts.saturating_sub(125_000)),
        indisunique: object.is_multiple_of(5),
        indisprimary: object.is_multiple_of(11),
        indisvalid: !object.is_multiple_of(101),
        indisexclusion: object.is_multiple_of(127),
        indisready: !object.is_multiple_of(103),
        amname: if object.is_multiple_of(19) {
            "hash"
        } else {
            "btree"
        }
        .to_owned(),
        indexdef: Some(format!(
            "CREATE INDEX index_{object:05} ON schema_{}.table_{:05} USING btree (id)",
            object % 32,
            object / 2,
        )),
        idx_blks_read: counter / 2,
        idx_blks_hit: counter.saturating_mul(4),
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the CPUFreq acceptance artifact measures bounded sysfs reads and the exact production WAL/ZMS path together"
)]
fn cpufreq_hour_reports_collection_and_production_writer_costs() {
    if std::env::var_os(CPUFREQ_COST_CHILD_ENV).is_none() {
        let executable = std::env::current_exe().expect("locate collector test binary");
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                CPUFREQ_COST_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CPUFREQ_COST_CHILD_ENV, "1")
            .output()
            .expect("run isolated CPUFreq cost child");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "isolated CPUFreq cost child exited with {}",
            output.status
        );
        return;
    }

    let sysfs = tempfile::tempdir().expect("create CPUFreq sysfs fixture");
    write_cpufreq_fixture(sysfs.path(), CPUFREQ_POLICY_COUNT);
    let sys = SysFs::new(sysfs.path().to_path_buf());
    let collection_cpu_before = self_cpu_ticks();
    let collection_started = std::time::Instant::now();
    for sample in 0..CPUFREQ_SNAPSHOTS_PER_HOUR {
        let observed =
            cpufreq::collect(&sys, sample % 6 == 0, true).expect("collect bounded CPUFreq fixture");
        assert_eq!(
            observed.policies.len(),
            if sample % 6 == 0 {
                CPUFREQ_POLICY_COUNT
            } else {
                0
            }
        );
        assert_eq!(observed.samples.len(), CPUFREQ_POLICY_COUNT);
    }
    let collection_elapsed_us =
        u64::try_from(collection_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let collection_cpu_ticks = self_cpu_ticks().saturating_sub(collection_cpu_before);
    let collection_rss_kib = peak_rss_kib();

    let directory = tempfile::tempdir().expect("create CPUFreq cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open CPUFreq journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let writer_cpu_before = self_cpu_ticks();
    let writer_started = std::time::Instant::now();
    let mut policy_raw_section_bytes = 0_u64;
    let mut sample_raw_section_bytes = 0_u64;
    for sample in 0..CPUFREQ_SNAPSHOTS_PER_HOUR {
        let sample_i64 = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample_i64.saturating_mul(10_000_000));
        let source = segment
            .interner_mut()
            .intern(b"cpuinfo_avg_freq")
            .map(|id| StrId(id.get()))
            .expect("intern CPUFreq source");
        let samples = (0..CPUFREQ_POLICY_COUNT)
            .map(|policy| {
                let policy_id = i32::try_from(policy).expect("policy count fits i32");
                OsCpufreq {
                    ts: Ts(ts),
                    policy_id,
                    actual_source: Some(source),
                    actual_frequency_hz: Some(
                        2_000_000_000_i64
                            .saturating_add(i64::from(policy_id).saturating_mul(1_000_000)),
                    ),
                    scaling_cur_freq_hz: Some(1_900_000_000),
                    scaling_min_freq_hz: Some(800_000_000),
                    scaling_max_freq_hz: Some(3_600_000_000),
                    online_cpus: Some(1),
                    scope: 0,
                }
            })
            .collect::<Vec<_>>();
        let policies = if sample % 6 == 0 {
            let driver = segment
                .interner_mut()
                .intern(b"intel_pstate")
                .map(|id| StrId(id.get()))
                .expect("intern CPUFreq driver");
            (0..CPUFREQ_POLICY_COUNT)
                .map(|policy| {
                    let policy_id = i32::try_from(policy).expect("policy count fits i32");
                    let related = segment
                        .interner_mut()
                        .intern(policy.to_string().as_bytes())
                        .map(|id| StrId(id.get()))
                        .expect("intern related CPUs");
                    OsCpufreqPolicy {
                        ts: Ts(ts),
                        policy_id,
                        related_cpus: Some(related),
                        scaling_driver: Some(driver),
                        actual_source: Some(source),
                        cpuinfo_min_freq_hz: Some(800_000_000),
                        cpuinfo_max_freq_hz: Some(3_600_000_000),
                        scope: 0,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut buffers = SectionBuffers::new();
        push_os_sources(&mut buffers, &OsSources::cpufreq_only(policies, samples))
            .expect("buffer CPUFreq rows");
        let flushed = encode_window(buffers, segment.interner()).expect("encode CPUFreq window");
        for section in &flushed.summary.sections {
            let bytes = u64::try_from(section.body_bytes).unwrap_or(u64::MAX);
            match section.type_id {
                CPUFREQ_POLICY_TYPE_ID => {
                    policy_raw_section_bytes = policy_raw_section_bytes.saturating_add(bytes);
                }
                CPUFREQ_TYPE_ID => {
                    sample_raw_section_bytes = sample_raw_section_bytes.saturating_add(bytes);
                }
                _ => {}
            }
        }
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            false,
            &flushed,
        )
        .expect("append CPUFreq window");
        assert!(completed.is_empty());
    }
    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write CPUFreq cost segment");
    let writer_elapsed_us = u64::try_from(writer_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let writer_cpu_ticks = self_cpu_ticks().saturating_sub(writer_cpu_before);
    let writer_rss_kib = peak_rss_kib();
    let reader = Reader::open(directory.path()).expect("open CPUFreq reader");
    let listing = reader.segments(..).expect("list CPUFreq segment");
    let stored = reader
        .open_segment(listing.segments.first().expect("one CPUFreq segment"))
        .expect("open CPUFreq segment");
    let policy_rows = stored
        .rows(CPUFREQ_POLICY_TYPE_ID)
        .expect("read CPUFreq policy rows");
    let sample_rows = stored
        .rows(CPUFREQ_TYPE_ID)
        .expect("read CPUFreq sample rows");
    let zms_section_bytes = |wanted| {
        stored
            .sections()
            .find(|(type_id, _)| *type_id == wanted)
            .map(|(_, section)| section.bytes)
            .expect("CPUFreq section catalogued")
    };
    let policy_zms_section_bytes = zms_section_bytes(CPUFREQ_POLICY_TYPE_ID);
    let sample_zms_section_bytes = zms_section_bytes(CPUFREQ_TYPE_ID);
    let section_bytes = policy_zms_section_bytes.saturating_add(sample_zms_section_bytes);
    let zms_bytes = std::fs::metadata(path).expect("stat CPUFreq segment").len();
    assert_eq!(policy_rows.len(), CPUFREQ_POLICY_COUNT * 60);
    assert_eq!(
        sample_rows.len(),
        CPUFREQ_POLICY_COUNT * CPUFREQ_SNAPSHOTS_PER_HOUR
    );
    assert!(raw_wal_bytes < 32 * 1024 * 1024);
    assert!(zms_bytes < 4 * 1024 * 1024);
    assert!(collection_rss_kib > 0);
    assert!(writer_rss_kib > 0);
    #[cfg(not(debug_assertions))]
    assert!(writer_rss_kib <= 25_600);
    println!(
        "os_cpufreq_cost policies={} samples={} raw_wal_bytes={} policy_raw_section_bytes={} sample_raw_section_bytes={} policy_zms_section_bytes={} sample_zms_section_bytes={} marginal_zms_bytes={} zms_bytes={} collection_elapsed_us={} collection_cpu_ticks={} collection_peak_rss_kib={} writer_elapsed_us={} writer_cpu_ticks={} writer_peak_rss_kib={}",
        policy_rows.len(),
        sample_rows.len(),
        raw_wal_bytes,
        policy_raw_section_bytes,
        sample_raw_section_bytes,
        policy_zms_section_bytes,
        sample_zms_section_bytes,
        section_bytes.saturating_add(2 * u64::try_from(ENTRY_LEN).unwrap_or(0)),
        zms_bytes,
        collection_elapsed_us,
        collection_cpu_ticks,
        collection_rss_kib,
        writer_elapsed_us,
        writer_cpu_ticks,
        writer_rss_kib,
    );
}

fn write_cpufreq_fixture(root: &Path, policies: usize) {
    let cpu_root = root.join("devices/system/cpu");
    std::fs::create_dir_all(&cpu_root).expect("create CPU sysfs root");
    std::fs::write(cpu_root.join("online"), format!("0-{}\n", policies - 1))
        .expect("write online CPUs");
    for policy in 0..policies {
        let path = cpu_root.join(format!("cpufreq/policy{policy}"));
        std::fs::create_dir_all(&path).expect("create CPUFreq policy");
        for (name, value) in [
            ("related_cpus", policy.to_string()),
            ("affected_cpus", policy.to_string()),
            ("scaling_driver", "intel_pstate".to_owned()),
            ("cpuinfo_avg_freq", "2400000".to_owned()),
            ("cpuinfo_min_freq", "800000".to_owned()),
            ("cpuinfo_max_freq", "3600000".to_owned()),
            ("scaling_cur_freq", "2200000".to_owned()),
            ("scaling_min_freq", "800000".to_owned()),
            ("scaling_max_freq", "3600000".to_owned()),
        ] {
            std::fs::write(path.join(name), value).expect("write CPUFreq attribute");
        }
    }
}

struct UserCostArtifact {
    rows: usize,
    raw_wal_bytes: usize,
    raw_section_bytes: u64,
    raw_dictionary_section_bytes: u64,
    user_zms_section_bytes: u64,
    dictionary_zms_section_bytes: u64,
    marginal_zms_bytes: u64,
    zms_bytes: u64,
    collection_elapsed_us: u64,
    collection_cpu_ticks: u64,
    collection_rss_baseline_kib: u64,
    collection_peak_rss_kib: u64,
    writer_elapsed_us: u64,
    writer_cpu_ticks: u64,
    writer_peak_rss_kib: u64,
}

fn passwd_fixture(entries: usize) -> (PasswdSnapshot, Vec<u32>) {
    use std::fmt::Write as _;

    let mut contents = String::with_capacity(entries.saturating_mul(48));
    let mut uids = Vec::with_capacity(entries);
    for index in 0..entries {
        let uid = 10_000_u32.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
        writeln!(
            contents,
            "user{index:04}:x:{uid}:{uid}::/srv/user{index:04}:/bin/false"
        )
        .expect("write passwd fixture row");
        uids.push(uid);
    }
    let file = tempfile::NamedTempFile::new().expect("create passwd fixture");
    std::fs::write(file.path(), contents).expect("write passwd fixture");
    let snapshot = PasswdSnapshot::read(file.path()).expect("read passwd fixture");
    assert_eq!(snapshot.len(), entries);
    (snapshot, uids)
}

#[allow(
    clippy::too_many_lines,
    reason = "one measurement keeps capture, WAL, finished-segment, reader, CPU, and RSS accounting together"
)]
fn user_cost_artifact(
    passwd: PasswdSnapshot,
    uids: &[u32],
    samples: usize,
    observations_per_sample: usize,
) -> UserCostArtifact {
    let directory = tempfile::tempdir().expect("create user cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open user cost journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let mut references = UserReferences::with_passwd(passwd);
    let collection_rss_baseline_kib = peak_rss_kib();
    let mut collection_elapsed_us = 0_u64;
    let mut collection_cpu_ticks = 0_u64;
    let mut collection_peak_rss_kib = collection_rss_baseline_kib;
    let mut writer_elapsed_us = 0_u64;
    let mut writer_cpu_ticks = 0_u64;
    let mut raw_section_bytes = 0_u64;
    let mut raw_dictionary_section_bytes = 0_u64;
    let mut appended = 0_usize;

    for sample in 0..samples {
        let sample_i64 = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample_i64.saturating_mul(30_000_000));
        let observed = uids.iter().copied().cycle().take(observations_per_sample);
        let collection_cpu_before = self_cpu_ticks();
        let collection_started = std::time::Instant::now();
        let (rows, pending) = references.prepare_rows(segment.interner_mut(), 0, ts, observed);
        collection_elapsed_us = collection_elapsed_us.saturating_add(
            u64::try_from(collection_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
        collection_cpu_ticks = collection_cpu_ticks
            .saturating_add(self_cpu_ticks().saturating_sub(collection_cpu_before));
        if sample == 0 {
            collection_peak_rss_kib = peak_rss_kib();
        }
        if rows.is_empty() {
            continue;
        }
        let writer_cpu_before = self_cpu_ticks();
        let writer_started = std::time::Instant::now();
        appended = appended.saturating_add(rows.len());
        let mut buffers = SectionBuffers::new();
        push_os_sources(&mut buffers, &OsSources::users_only(rows))
            .expect("buffer user reference rows");
        let flushed = encode_window(buffers, segment.interner()).expect("encode user window");
        raw_section_bytes = raw_section_bytes.saturating_add(
            flushed
                .summary
                .sections
                .iter()
                .find(|section| section.type_id == USER_TYPE_ID)
                .map(|section| u64::try_from(section.body_bytes).unwrap_or(u64::MAX))
                .expect("user section encoded"),
        );
        raw_dictionary_section_bytes = raw_dictionary_section_bytes.saturating_add(
            flushed
                .summary
                .sections
                .iter()
                .find(|section| section.type_id == DICT_STRINGS_TYPE_ID)
                .map(|section| u64::try_from(section.body_bytes).unwrap_or(u64::MAX))
                .expect("user dictionary section encoded"),
        );
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            false,
            &flushed,
        )
        .expect("append user window");
        assert!(completed.is_empty());
        references.mark_recorded(&pending);
        writer_elapsed_us = writer_elapsed_us.saturating_add(
            u64::try_from(writer_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
        writer_cpu_ticks =
            writer_cpu_ticks.saturating_add(self_cpu_ticks().saturating_sub(writer_cpu_before));
    }

    let raw_wal_bytes = journal.bytes();
    let writer_cpu_before = self_cpu_ticks();
    let writer_started = std::time::Instant::now();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write user cost segment");
    writer_elapsed_us = writer_elapsed_us
        .saturating_add(u64::try_from(writer_started.elapsed().as_micros()).unwrap_or(u64::MAX));
    writer_cpu_ticks =
        writer_cpu_ticks.saturating_add(self_cpu_ticks().saturating_sub(writer_cpu_before));
    let writer_peak_rss_kib = peak_rss_kib();
    let reader = Reader::open(directory.path()).expect("open user cost reader");
    let listing = reader.segments(..).expect("list user cost segment");
    let stored = reader
        .open_segment(listing.segments.first().expect("one user cost segment"))
        .expect("open user cost segment");
    let rows = stored.rows(USER_TYPE_ID).expect("read user rows");
    let dictionary = stored.dictionary().expect("read user dictionary");
    for row in &rows {
        let Some(Cell::StrId(username)) = row.get("username") else {
            panic!("user name is a dictionary reference");
        };
        assert!(matches!(
            dictionary.resolve(*username),
            Some(Resolved::Str(_))
        ));
    }
    let section_bytes = |wanted| {
        stored
            .sections()
            .find(|(type_id, _)| *type_id == wanted)
            .map(|(_, section)| section.bytes)
            .expect("user artifact section catalogued")
    };
    let user_zms_section_bytes = section_bytes(USER_TYPE_ID);
    let dictionary_zms_section_bytes = section_bytes(DICT_STRINGS_TYPE_ID);
    let marginal_zms_bytes = user_zms_section_bytes
        .saturating_add(dictionary_zms_section_bytes)
        .saturating_add(2 * u64::try_from(ENTRY_LEN).unwrap_or(0));
    let zms_bytes = std::fs::metadata(path).expect("stat user segment").len();
    assert_eq!(rows.len(), appended);

    UserCostArtifact {
        rows: rows.len(),
        raw_wal_bytes,
        raw_section_bytes,
        raw_dictionary_section_bytes,
        user_zms_section_bytes,
        dictionary_zms_section_bytes,
        marginal_zms_bytes,
        zms_bytes,
        collection_elapsed_us,
        collection_cpu_ticks,
        collection_rss_baseline_kib,
        collection_peak_rss_kib,
        writer_elapsed_us,
        writer_cpu_ticks,
        writer_peak_rss_kib,
    }
}

fn print_user_cost(label: &str, artifact: &UserCostArtifact) {
    println!(
        "os_user_cost case={label} rows={} raw_wal_bytes={} raw_section_bytes={} raw_dictionary_section_bytes={} user_zms_section_bytes={} dictionary_zms_section_bytes={} marginal_zms_bytes={} zms_bytes={} collection_elapsed_us={} collection_cpu_ticks={} collection_rss_baseline_kib={} collection_peak_rss_kib={} collection_rss_growth_kib={} writer_elapsed_us={} writer_cpu_ticks={} writer_peak_rss_kib={}",
        artifact.rows,
        artifact.raw_wal_bytes,
        artifact.raw_section_bytes,
        artifact.raw_dictionary_section_bytes,
        artifact.user_zms_section_bytes,
        artifact.dictionary_zms_section_bytes,
        artifact.marginal_zms_bytes,
        artifact.zms_bytes,
        artifact.collection_elapsed_us,
        artifact.collection_cpu_ticks,
        artifact.collection_rss_baseline_kib,
        artifact.collection_peak_rss_kib,
        artifact
            .collection_peak_rss_kib
            .saturating_sub(artifact.collection_rss_baseline_kib),
        artifact.writer_elapsed_us,
        artifact.writer_cpu_ticks,
        artifact.writer_peak_rss_kib,
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the user-reference acceptance artifact compares bounded capture cases through the production WAL and ZMS path"
)]
fn process_user_references_report_production_storage_and_resource_costs() {
    if std::env::var_os(USER_COST_CHILD_ENV).is_none() {
        let executable = std::env::current_exe().expect("locate collector test binary");
        let output = std::process::Command::new(executable)
            .args(["--exact", USER_COST_TEST, "--nocapture", "--test-threads=1"])
            .env(USER_COST_CHILD_ENV, "1")
            .output()
            .expect("run isolated user cost child");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "isolated user cost child exited with {}",
            output.status
        );
        return;
    }

    let (shared_passwd, shared_uids) = passwd_fixture(1);
    let shared = user_cost_artifact(shared_passwd, &shared_uids, 120, 1_000);
    let (ordinary_passwd, ordinary_uids) = passwd_fixture(16);
    let ordinary = user_cost_artifact(ordinary_passwd, &ordinary_uids, 1, 16);
    let (high_passwd, high_uids) = passwd_fixture(4_096);
    let high = user_cost_artifact(high_passwd, &high_uids, 1, 4_096);

    let (unresolved_passwd, _unresolved_uids) = passwd_fixture(1);
    let mut unresolved = UserReferences::with_passwd(unresolved_passwd);
    let mut unresolved_interner =
        kronika_writer::Interner::new(kronika_format::DictLimits::default());
    let (unresolved_rows, unresolved_pending) =
        unresolved.prepare_rows(&mut unresolved_interner, 0, BASE_TS, [u32::MAX]);
    assert!(unresolved_rows.is_empty());
    assert!(unresolved_pending.is_empty());

    let malformed_file = tempfile::NamedTempFile::new().expect("create malformed fixture");
    std::fs::write(
        malformed_file.path(),
        b"bad\nvalid:x:12000:12000::/:/bin/false\n",
    )
    .expect("write malformed fixture");
    let malformed = PasswdSnapshot::read(malformed_file.path()).expect("read malformed fixture");
    assert_eq!(malformed.len(), 1);
    assert_eq!(malformed.rejected_lines(), 1);

    let oversized_file = tempfile::NamedTempFile::new().expect("create oversized fixture");
    std::fs::write(
        oversized_file.path(),
        vec![b'x'; MAX_PASSWD_BYTES.saturating_add(1)],
    )
    .expect("write oversized fixture");
    assert!(PasswdSnapshot::read(oversized_file.path()).is_err());

    assert_eq!(shared.rows, 1);
    assert_eq!(ordinary.rows, 16);
    assert_eq!(high.rows, 4_096);
    assert!(shared.raw_wal_bytes < 32 * 1024);
    assert!(ordinary.raw_wal_bytes < 32 * 1024);
    assert!(high.raw_wal_bytes < 512 * 1024);
    assert!(shared.zms_bytes < 32 * 1024);
    assert!(ordinary.zms_bytes < 32 * 1024);
    assert!(high.zms_bytes < 512 * 1024);
    assert!(
        high.collection_peak_rss_kib
            .saturating_sub(high.collection_rss_baseline_kib)
            <= 10 * 1_024
    );
    print_user_cost("shared", &shared);
    print_user_cost("ordinary", &ordinary);
    print_user_cost("high_cardinality", &high);
    println!(
        "os_user_cost shared_process_observations={} repeated_samples={} unresolved_rows={} malformed_rejected={} oversized_rows=0",
        1_000 * 120,
        120,
        unresolved_rows.len(),
        malformed.rejected_lines(),
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the storage acceptance artifact measures exact layouts through the production writer"
)]
fn storage_hour_reports_collection_and_production_writer_costs() {
    let sysfs = tempfile::tempdir().expect("create block topology sysfs fixture");
    write_block_topology_fixture(sysfs.path(), STORAGE_EDGES_PER_SNAPSHOT);
    let sys = SysFs::new(sysfs.path().to_path_buf());
    let collection_cpu_before = self_cpu_ticks();
    let collection_started = std::time::Instant::now();
    for _ in 0..STORAGE_SNAPSHOTS_PER_HOUR {
        assert_eq!(
            block_topology::collect(&sys)
                .expect("collect bounded block topology fixture")
                .len(),
            STORAGE_EDGES_PER_SNAPSHOT
        );
    }
    let collection_elapsed_us =
        u64::try_from(collection_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let collection_cpu_ticks = self_cpu_ticks().saturating_sub(collection_cpu_before);
    let collection_rss_kib = peak_rss_kib();

    let directory = tempfile::tempdir().expect("create storage cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open storage cost journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let writer_cpu_before = self_cpu_ticks();
    let writer_started = std::time::Instant::now();
    let mut mount_raw_section_bytes = 0_u64;
    let mut topology_raw_section_bytes = 0_u64;
    for sample in 0..STORAGE_SNAPSHOTS_PER_HOUR {
        let sample_i64 = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample_i64.saturating_mul(60_000_000));
        let fstype = segment
            .interner_mut()
            .intern(b"ext4")
            .map(|id| StrId(id.get()))
            .expect("intern filesystem type");
        let root = segment
            .interner_mut()
            .intern(b"/")
            .map(|id| StrId(id.get()))
            .expect("intern filesystem root");
        let mounts = (0..STORAGE_MOUNTS_PER_SNAPSHOT)
            .map(|mount| {
                let minor = i32::try_from(mount + 1).expect("mount count fits i32");
                let mount_point = segment
                    .interner_mut()
                    .intern(format!("/srv/data/{mount}").as_bytes())
                    .map(|id| StrId(id.get()))
                    .expect("intern mount point");
                let source = segment
                    .interner_mut()
                    .intern(format!("/dev/nvme0n1p{}", mount + 1).as_bytes())
                    .map(|id| StrId(id.get()))
                    .expect("intern mount source");
                OsMountinfo {
                    ts: Ts(ts),
                    major: 259,
                    minor,
                    mount_point,
                    root,
                    fstype,
                    source,
                    is_k8s_infra: false,
                    total_bytes: Some(1_099_511_627_776),
                    free_bytes: Some(
                        824_633_720_832_i64.saturating_sub(sample_i64.saturating_mul(1_048_576)),
                    ),
                    total_inodes: Some(67_108_864),
                    available_inodes: Some(
                        60_000_000_i64.saturating_sub(sample_i64.saturating_mul(10)),
                    ),
                    scope: 0,
                }
            })
            .collect::<Vec<_>>();
        let edges = (0..STORAGE_EDGES_PER_SNAPSHOT)
            .map(|edge| OsBlockTopology {
                ts: Ts(ts),
                major: 259,
                minor: i32::try_from(edge + 1).expect("edge count fits i32"),
                parent_major: 259,
                parent_minor: 0,
                scope: 0,
            })
            .collect::<Vec<_>>();
        let mut buffers = SectionBuffers::new();
        push_os_sources(&mut buffers, &OsSources::storage_only(mounts, edges))
            .expect("buffer storage rows");
        let flushed = encode_window(buffers, segment.interner()).expect("encode storage window");
        for section in &flushed.summary.sections {
            let bytes = u64::try_from(section.body_bytes).unwrap_or(u64::MAX);
            match section.type_id {
                MOUNTINFO_TYPE_ID => {
                    mount_raw_section_bytes = mount_raw_section_bytes.saturating_add(bytes);
                }
                BLOCK_TOPOLOGY_TYPE_ID => {
                    topology_raw_section_bytes = topology_raw_section_bytes.saturating_add(bytes);
                }
                _ => {}
            }
        }
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            false,
            &flushed,
        )
        .expect("append storage window");
        assert!(completed.is_empty());
    }
    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write storage cost segment");
    let writer_elapsed_us = u64::try_from(writer_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let writer_cpu_ticks = self_cpu_ticks().saturating_sub(writer_cpu_before);
    let writer_rss_kib = peak_rss_kib();
    let reader = Reader::open(directory.path()).expect("open storage reader");
    let listing = reader.segments(..).expect("list storage segment");
    let stored = reader
        .open_segment(listing.segments.first().expect("one storage segment"))
        .expect("open storage segment");
    let mount_rows = stored.rows(MOUNTINFO_TYPE_ID).expect("read mount rows");
    let topology_rows = stored
        .rows(BLOCK_TOPOLOGY_TYPE_ID)
        .expect("read block topology rows");
    let zms_section_bytes = |wanted| {
        stored
            .sections()
            .find(|(type_id, _)| *type_id == wanted)
            .map(|(_, section)| section.bytes)
            .expect("storage section catalogued")
    };
    let mount_zms_section_bytes = zms_section_bytes(MOUNTINFO_TYPE_ID);
    let topology_zms_section_bytes = zms_section_bytes(BLOCK_TOPOLOGY_TYPE_ID);
    let section_bytes = mount_zms_section_bytes.saturating_add(topology_zms_section_bytes);
    let zms_bytes = std::fs::metadata(path).expect("stat storage segment").len();
    assert_eq!(
        mount_rows.len(),
        STORAGE_MOUNTS_PER_SNAPSHOT * STORAGE_SNAPSHOTS_PER_HOUR
    );
    assert_eq!(
        topology_rows.len(),
        STORAGE_EDGES_PER_SNAPSHOT * STORAGE_SNAPSHOTS_PER_HOUR
    );
    assert!(raw_wal_bytes < 16 * 1024 * 1024);
    assert!(zms_bytes < 2 * 1024 * 1024);
    println!(
        "os_storage_cost mounts={} topology_edges={} raw_wal_bytes={} mount_raw_section_bytes={} topology_raw_section_bytes={} mount_zms_section_bytes={} topology_zms_section_bytes={} marginal_zms_bytes={} zms_bytes={} collection_elapsed_us={} collection_cpu_ticks={} collection_peak_rss_kib={} writer_elapsed_us={} writer_cpu_ticks={} writer_peak_rss_kib={}",
        mount_rows.len(),
        topology_rows.len(),
        raw_wal_bytes,
        mount_raw_section_bytes,
        topology_raw_section_bytes,
        mount_zms_section_bytes,
        topology_zms_section_bytes,
        section_bytes.saturating_add(2 * u64::try_from(ENTRY_LEN).unwrap_or(0)),
        zms_bytes,
        collection_elapsed_us,
        collection_cpu_ticks,
        collection_rss_kib,
        writer_elapsed_us,
        writer_cpu_ticks,
        writer_rss_kib,
    );
}

fn write_block_topology_fixture(root: &Path, partitions: usize) {
    let block = root.join("devices/pci/block/nvme0n1");
    std::fs::create_dir_all(root.join("dev/block")).expect("create dev block directory");
    std::fs::create_dir_all(&block).expect("create parent block device");
    std::fs::write(block.join("dev"), "259:0\n").expect("write parent device identity");
    for partition in 0..partitions {
        let minor = partition + 1;
        let name = format!("nvme0n1p{minor}");
        let path = block.join(&name);
        std::fs::create_dir_all(&path).expect("create partition directory");
        std::fs::write(path.join("partition"), format!("{minor}\n"))
            .expect("write partition marker");
        std::os::unix::fs::symlink(
            format!("../../devices/pci/block/nvme0n1/{name}"),
            root.join(format!("dev/block/259:{minor}")),
        )
        .expect("link partition device");
    }
}

fn write_cgroup_cost_fixture(root: &Path) -> (SysFs, Vec<String>) {
    use std::fmt::Write as _;

    let cgroup_root = root.join("fs/cgroup");
    std::fs::create_dir_all(&cgroup_root).expect("create cgroup cost root");
    std::fs::write(
        cgroup_root.join("cgroup.controllers"),
        "cpu memory io pids\n",
    )
    .expect("write cgroup cost controllers");
    let mut memberships = Vec::with_capacity(CGROUP_CANDIDATE_COUNT);
    for candidate in 0..CGROUP_CANDIDATE_COUNT {
        let path = format!(
            "/kubepods.slice/kubepods-burstable.slice/pod-{candidate:04}/container-{candidate:04}"
        );
        memberships.push(format!("0::{path}\n"));
        let workload = cgroup_root.join(path.trim_start_matches('/'));
        std::fs::create_dir_all(&workload).expect("create cgroup cost workload");
        std::fs::write(
            workload.join("cpu.stat"),
            "usage_usec 100\nuser_usec 60\nsystem_usec 30\nnr_throttled 2\nthrottled_usec 5\n",
        )
        .expect("write cgroup cost cpu.stat");
        std::fs::write(workload.join("cpu.max"), "200000 100000\n")
            .expect("write cgroup cost cpu.max");
        std::fs::write(workload.join("memory.current"), "536870912\n")
            .expect("write cgroup cost memory.current");
        std::fs::write(workload.join("memory.max"), "1073741824\n")
            .expect("write cgroup cost memory.max");
        std::fs::write(
            workload.join("memory.stat"),
            "anon 268435456\nfile 134217728\nkernel 67108864\nslab 33554432\n",
        )
        .expect("write cgroup cost memory.stat");
        std::fs::write(
            workload.join("memory.events"),
            "low 0\nhigh 1\nmax 2\noom 0\noom_kill 0\n",
        )
        .expect("write cgroup cost memory.events");
        std::fs::write(workload.join("pids.current"), "16\n")
            .expect("write cgroup cost pids.current");
        std::fs::write(workload.join("pids.max"), "256\n").expect("write cgroup cost pids.max");
        let mut io_stat = String::new();
        for device in 0..CGROUP_DEVICES_PER_CANDIDATE {
            let minor = candidate
                .saturating_mul(CGROUP_DEVICES_PER_CANDIDATE)
                .saturating_add(device);
            writeln!(io_stat, "8:{minor} rbytes=1000 wbytes=2000 rios=10 wios=20")
                .expect("write cgroup cost I/O fixture");
        }
        std::fs::write(workload.join("io.stat"), io_stat).expect("write cgroup cost io.stat");
    }
    (SysFs::new(root.to_path_buf()), memberships)
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the bounded cgroup acceptance artifact measures exact cgroupfs reads and the production WAL/ZMS path together"
)]
fn bounded_cgroup_hour_reports_collection_and_production_writer_costs() {
    if std::env::var_os(CGROUP_COST_CHILD_ENV).is_none() {
        let executable = std::env::current_exe().expect("locate collector test binary");
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                CGROUP_COST_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CGROUP_COST_CHILD_ENV, "1")
            .output()
            .expect("run isolated cgroup cost child");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            output.status.success(),
            "isolated cgroup cost child exited with {}",
            output.status
        );
        return;
    }

    let sysfs = tempfile::tempdir().expect("create cgroup cost sysfs fixture");
    let (sys, memberships) = write_cgroup_cost_fixture(sysfs.path());
    let collection_cpu_before = self_cpu_ticks();
    let collection_started = std::time::Instant::now();
    for sample in 0..CGROUP_SNAPSHOTS_PER_HOUR {
        let rows = cgroup::collect_workload_memberships(
            memberships.iter().map(String::as_str),
            &sys,
            i64::try_from(sample).expect("sample count fits i64"),
            100,
        )
        .expect("collect bounded cgroup fixture");
        assert_eq!(rows.cpu.len(), CGROUP_CANDIDATE_COUNT);
        assert_eq!(rows.memory.len(), CGROUP_CANDIDATE_COUNT);
        assert_eq!(rows.pids.len(), CGROUP_CANDIDATE_COUNT);
        assert_eq!(rows.io.len(), cgroup::MAX_CGROUP_IO_ROWS);
        assert!(!rows.io_omitted);
    }
    let collection_elapsed_us =
        u64::try_from(collection_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let collection_cpu_ticks = self_cpu_ticks().saturating_sub(collection_cpu_before);
    let collection_rss_kib = peak_rss_kib();
    assert!(collection_rss_kib > 0);

    let directory = tempfile::tempdir().expect("create cgroup cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open cgroup cost journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();
    let writer_cpu_before = self_cpu_ticks();
    let writer_started = std::time::Instant::now();
    let mut raw_section_bytes = BTreeMap::<u32, u64>::new();
    let mut raw_wal_bytes = 0_u64;
    let mut completed_paths = Vec::new();
    for sample in 0..CGROUP_SNAPSHOTS_PER_HOUR {
        let sample_i64 = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample_i64.saturating_mul(30_000_000));
        let mut cpu = Vec::with_capacity(CGROUP_CANDIDATE_COUNT);
        let mut memory = Vec::with_capacity(CGROUP_CANDIDATE_COUNT);
        let mut io = Vec::with_capacity(cgroup::MAX_CGROUP_IO_ROWS);
        let mut pids = Vec::with_capacity(CGROUP_CANDIDATE_COUNT);
        for candidate in 0..CGROUP_CANDIDATE_COUNT {
            let path = format!(
                "/kubepods.slice/kubepods-burstable.slice/pod-{candidate:04}/container-{candidate:04}"
            );
            let path = segment
                .interner_mut()
                .intern(path.as_bytes())
                .map(|id| StrId(id.get()))
                .expect("intern cgroup cost path");
            let candidate_i64 = i64::try_from(candidate).expect("candidate count fits i64");
            let counter = sample_i64
                .saturating_mul(10_000)
                .saturating_add(candidate_i64);
            cpu.push(OsCgroupCpu {
                ts: Ts(ts),
                cgroup_path: path,
                usage_usec: counter,
                user_usec: counter.saturating_mul(3) / 5,
                system_usec: counter.saturating_mul(3) / 10,
                throttled_usec: counter / 100,
                nr_throttled: counter / 1000,
                quota_usec: 200_000,
                period_usec: 100_000,
                scope: 0,
            });
            memory.push(OsCgroupMemory {
                ts: Ts(ts),
                cgroup_path: path,
                current: 536_870_912_i64.saturating_add(candidate_i64),
                max: Some(1_073_741_824),
                anon: 268_435_456,
                file: 134_217_728,
                kernel: 67_108_864,
                slab: 33_554_432,
                low_events: 0,
                high_events: sample_i64,
                max_events: sample_i64 / 2,
                oom_events: 0,
                oom_kill: 0,
                scope: 0,
            });
            pids.push(OsCgroupPids {
                ts: Ts(ts),
                cgroup_path: path,
                current: 16,
                max: Some(256),
                scope: 0,
            });
            for device in 0..CGROUP_DEVICES_PER_CANDIDATE {
                let minor = candidate
                    .saturating_mul(CGROUP_DEVICES_PER_CANDIDATE)
                    .saturating_add(device);
                io.push(OsCgroupIo {
                    ts: Ts(ts),
                    cgroup_path: path,
                    major: 8,
                    minor: u32::try_from(minor).expect("device count fits u32"),
                    rbytes: Some(counter.saturating_mul(1000)),
                    wbytes: Some(counter.saturating_mul(2000)),
                    rios: Some(counter.saturating_mul(10)),
                    wios: Some(counter.saturating_mul(20)),
                    scope: 0,
                });
            }
        }
        let mut buffers = SectionBuffers::new();
        push_os_sources(
            &mut buffers,
            &OsSources::cgroups_only(cpu, memory, io, pids),
        )
        .expect("buffer cgroup rows");
        let flushed = encode_window(buffers, segment.interner()).expect("encode cgroup window");
        for section in &flushed.summary.sections {
            let total = raw_section_bytes.entry(section.type_id).or_default();
            *total = total.saturating_add(u64::try_from(section.body_bytes).unwrap_or(u64::MAX));
        }
        raw_wal_bytes = raw_wal_bytes
            .saturating_add(u64::try_from(flushed.summary.part_bytes).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(FRAME_HEADER_LEN).unwrap_or(u64::MAX));
        let forced = sample % 60 == 59;
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            forced,
            &flushed,
        )
        .expect("append cgroup window");
        if forced {
            assert_eq!(completed.len(), 1);
            completed_paths.push(completed[0].0.clone());
        } else {
            assert!(completed.is_empty());
        }
    }
    assert_eq!(completed_paths.len(), 2);
    let writer_elapsed_us = u64::try_from(writer_started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let writer_cpu_ticks = self_cpu_ticks().saturating_sub(writer_cpu_before);
    let writer_rss_kib = peak_rss_kib();
    let reader = Reader::open(directory.path()).expect("open cgroup reader");
    let listing = reader.segments(..).expect("list cgroup segment");
    assert_eq!(listing.segments.len(), 2);
    let expected_candidates = CGROUP_CANDIDATE_COUNT * CGROUP_SNAPSHOTS_PER_HOUR;
    let expected_io = cgroup::MAX_CGROUP_IO_ROWS * CGROUP_SNAPSHOTS_PER_HOUR;
    let mut stored_rows = BTreeMap::<u32, usize>::new();
    let mut section_bytes = 0_u64;
    for reference in &listing.segments {
        let stored = reader.open_segment(reference).expect("open cgroup segment");
        for wanted in [
            CGROUP_CPU_TYPE_ID,
            CGROUP_MEMORY_TYPE_ID,
            CGROUP_IO_TYPE_ID,
            CGROUP_PIDS_TYPE_ID,
        ] {
            let rows = stored.rows(wanted).expect("read cgroup rows").len();
            let total = stored_rows.entry(wanted).or_default();
            *total = total.saturating_add(rows);
            section_bytes = section_bytes.saturating_add(
                stored
                    .sections()
                    .find(|(type_id, _)| *type_id == wanted)
                    .map(|(_, section)| section.bytes)
                    .expect("cgroup section catalogued"),
            );
        }
    }
    assert_eq!(
        stored_rows.get(&CGROUP_CPU_TYPE_ID).copied(),
        Some(expected_candidates)
    );
    assert_eq!(
        stored_rows.get(&CGROUP_MEMORY_TYPE_ID).copied(),
        Some(expected_candidates)
    );
    assert_eq!(
        stored_rows.get(&CGROUP_PIDS_TYPE_ID).copied(),
        Some(expected_candidates)
    );
    assert_eq!(
        stored_rows.get(&CGROUP_IO_TYPE_ID).copied(),
        Some(expected_io)
    );
    let zms_bytes = completed_paths
        .iter()
        .map(|path| std::fs::metadata(path).expect("stat cgroup segment").len())
        .sum::<u64>();
    let marginal_zms_bytes = section_bytes.saturating_add(
        u64::try_from(completed_paths.len().saturating_mul(4)).unwrap_or(u64::MAX)
            * u64::try_from(ENTRY_LEN).unwrap_or(0),
    );
    assert!(raw_wal_bytes < 64 * 1024 * 1024);
    assert!(zms_bytes < 16 * 1024 * 1024);
    assert!(writer_rss_kib > 0);
    #[cfg(not(debug_assertions))]
    assert!(writer_rss_kib <= 25_600);
    println!(
        "os_cgroup_cost environment=container candidates={} io_rows={} total_rows={} raw_wal_bytes={} cpu_raw_section_bytes={} memory_raw_section_bytes={} io_raw_section_bytes={} pids_raw_section_bytes={} zms_section_bytes={} marginal_zms_bytes={} zms_bytes={} machine_raw_wal_bytes=0 machine_marginal_zms_bytes=0 raw_wal_reduction_bytes={} marginal_zms_reduction_bytes={} collection_elapsed_us={} collection_cpu_ticks={} collection_peak_rss_kib={} writer_elapsed_us={} writer_cpu_ticks={} writer_peak_rss_kib={}",
        expected_candidates,
        expected_io,
        expected_candidates
            .saturating_mul(3)
            .saturating_add(expected_io),
        raw_wal_bytes,
        raw_section_bytes
            .get(&CGROUP_CPU_TYPE_ID)
            .copied()
            .unwrap_or_default(),
        raw_section_bytes
            .get(&CGROUP_MEMORY_TYPE_ID)
            .copied()
            .unwrap_or_default(),
        raw_section_bytes
            .get(&CGROUP_IO_TYPE_ID)
            .copied()
            .unwrap_or_default(),
        raw_section_bytes
            .get(&CGROUP_PIDS_TYPE_ID)
            .copied()
            .unwrap_or_default(),
        section_bytes,
        marginal_zms_bytes,
        zms_bytes,
        raw_wal_bytes,
        marginal_zms_bytes,
        collection_elapsed_us,
        collection_cpu_ticks,
        collection_rss_kib,
        writer_elapsed_us,
        writer_cpu_ticks,
        writer_rss_kib,
    );
}

#[test]
fn cgroup_context_hour_reports_raw_and_finished_costs() {
    let directory = tempfile::tempdir().expect("create cgroup context cost directory");
    let writer = owner(directory.path());
    let mut journal =
        Journal::open(&writer, JournalConfig::default()).expect("open cgroup context journal");
    let config = config(directory.path(), u64::MAX);
    let mut segment = SegmentState::default();

    for sample in 0..CGROUP_CONTEXT_SNAPSHOTS_PER_HOUR {
        let sample = i64::try_from(sample).expect("sample count fits i64");
        let ts = BASE_TS.saturating_add(sample.saturating_mul(10_000_000));
        let path = segment
            .interner_mut()
            .intern(b"/kubepods/pod-a/container-a")
            .map(|id| StrId(id.get()))
            .expect("intern cgroup path");
        let mut buffers = SectionBuffers::new();
        let sources = OsSources::cgroup_context_only(OsCgroupContext {
            ts: Ts(ts),
            cgroup_version: 2,
            cpu_path: Some(path),
            memory_path: Some(path),
            io_path: Some(path),
            cpuset_cpus: Some(2),
            effective_cpu_quota_usec: Some(150_000),
            effective_cpu_period_usec: Some(100_000),
            effective_memory_max: Some(536_870_912),
            scope: 3,
        });
        push_os_sources(&mut buffers, &sources).expect("buffer cgroup context");
        let flushed = encode_window(buffers, segment.interner()).expect("encode cgroup context");
        let completed = append_window_and_maybe_close(
            &mut journal,
            &writer,
            &config,
            &mut segment,
            ts,
            false,
            &flushed,
        )
        .expect("append cgroup context");
        assert!(completed.is_empty());
    }

    let raw_wal_bytes = journal.bytes();
    let path = close_open_segment(&mut journal, &writer, &mut segment, "test-end")
        .expect("write cgroup context cost segment");
    let reader = Reader::open(directory.path()).expect("open cgroup context cost reader");
    let listing = reader
        .segments(..)
        .expect("list cgroup context cost segment");
    let reference = listing
        .segments
        .first()
        .expect("one cgroup context segment");
    let stored = reader
        .open_segment(reference)
        .expect("open finished cgroup context segment");
    let rows = stored
        .rows(CGROUP_CONTEXT_TYPE_ID)
        .expect("read cgroup context rows");
    let section = stored
        .sections()
        .find(|(type_id, _section)| *type_id == CGROUP_CONTEXT_TYPE_ID)
        .map(|(_type_id, section)| section)
        .expect("cgroup context section is catalogued");
    let zms_bytes = std::fs::metadata(path)
        .expect("stat cgroup context segment")
        .len();
    let marginal_zms_bytes = section
        .bytes
        .saturating_add(u64::try_from(ENTRY_LEN).expect("catalog entry length fits u64"));

    assert_eq!(listing.segments.len(), 1);
    assert_eq!(stored.window_count(), 360);
    assert_eq!(rows.len(), CGROUP_CONTEXT_SNAPSHOTS_PER_HOUR);
    assert_cgroup_context_values(rows.first().expect("one cgroup context row"));
    let dictionary = stored.dictionary().expect("read cgroup context dictionary");
    for field in ["cpu_path", "memory_path", "io_path"] {
        let Some(Cell::StrId(path)) = rows.first().and_then(|row| row.get(field)) else {
            panic!("cgroup context {field} must be persisted");
        };
        match dictionary.resolve(*path) {
            Some(Resolved::Str(actual)) => {
                assert_eq!(actual, b"/kubepods/pod-a/container-a");
            }
            Some(Resolved::Blob(_)) => panic!("cgroup context path belongs in dict.strings"),
            None => panic!("cgroup context {field} id resolves"),
        }
    }
    assert!(raw_wal_bytes < 1024 * 1024);
    assert!(section.bytes < 8 * 1024);
    assert!(zms_bytes < 16 * 1024);
    println!(
        "os_cgroup_context_cost rows={} raw_wal_bytes={} section_bytes={} marginal_zms_bytes={} zms_bytes={}",
        rows.len(),
        raw_wal_bytes,
        section.bytes,
        marginal_zms_bytes,
        zms_bytes
    );
}

fn assert_cgroup_context_values(row: &Row) {
    for (field, expected) in [
        ("cpuset_cpus", 2),
        ("effective_cpu_quota_usec", 150_000),
        ("effective_cpu_period_usec", 100_000),
        ("effective_memory_max", 536_870_912),
    ] {
        assert_eq!(row.get(field), Some(&Cell::I64(expected)));
    }
}
