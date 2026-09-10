use super::*;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use kronika_layout::{DataRoot, LayoutLimits};
use kronika_reader::{Cell, Reader, Resolved};
use kronika_source_os::cgroup::discovery::walk_visible_v2;
use kronika_writer::JournalConfig;

use crate::config::CollectorMode;
use crate::scheduler::Intervals;
use crate::segments::close_open_segment;

const SCAN_TS: i64 = 1_788_000_000_000_000;

fn config(storage_dir: &Path) -> Config {
    Config {
        // Storage fixtures supply their own OS rows and never probe the test host.
        mode: CollectorMode::Postgresql,
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

fn fixture() -> (tempfile::TempDir, ProcFs, SysFs) {
    let temp = tempfile::tempdir().expect("fixture");
    let proc = temp.path().join("proc");
    let sys = temp.path().join("sys");
    std::fs::create_dir_all(proc.join("self")).expect("proc fixture");
    std::fs::create_dir_all(sys.join("fs/cgroup")).expect("cgroup fixture");
    std::fs::write(
        proc.join("self/mountinfo"),
        format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n",
            sys.join("fs/cgroup").display()
        ),
    )
    .expect("mountinfo");
    for index in 0..600 {
        let group = sys.join(format!("fs/cgroup/group-{index}"));
        std::fs::create_dir(&group).expect("group directory");
        std::fs::write(
            group.join("cpu.stat"),
            format!("usage_usec {index}\nuser_usec 0\nsystem_usec 0\n"),
        )
        .expect("CPU counters");
    }
    let mut io = String::new();
    for minor in 0..1200 {
        writeln!(io, "8:{minor} rbytes={minor} wbytes=2 rios=3 wios=4").expect("I/O line");
    }
    std::fs::write(sys.join("fs/cgroup/io.stat"), io).expect("I/O counters");
    (temp, ProcFs::new(proc), SysFs::new(sys))
}

fn open_journal(path: &Path, max_parts: usize) -> (WriterOwner, Journal) {
    std::fs::create_dir(path).expect("storage");
    let owner = DataRoot::open(path)
        .expect("data root")
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let journal = Journal::open(
        &owner,
        JournalConfig {
            max_parts,
            ..JournalConfig::default()
        },
    )
    .expect("journal");
    (owner, journal)
}

fn collect(
    proc: &ProcFs,
    sys: &SysFs,
    config: &Config,
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
) -> CgroupPass {
    let mut sched = Scheduler::new(Intervals::default());
    let mut pass = CgroupPass::default();
    let mut appender = Appender {
        config,
        in_container: false,
        journal,
        owner,
        segment,
        sched: &mut sched,
        opening_settings: &[],
        previous_open_ts: None,
        portion: Portion::default(),
    };
    pass.stats = walk_visible_v2(proc, sys, SCAN_TS, |row| {
        appender
            .accept(&row, false, &mut pass)
            .map_err(io::Error::other)
    })
    .expect("source walker and production append");
    appender.flush(&mut pass).expect("last portion");
    assert_eq!(pass.stats.groups, 601);
    assert_eq!(pass.stats.io_rows, 1200);
    assert_eq!(pass.stats.metric_files_read, 601);
    assert!(pass.peak_groups <= GROUPS_PER_PORTION);
    assert!(pass.peak_io_rows <= IO_PER_PORTION);
    assert!(pass.peak_string_bytes <= STRING_BYTES_PER_PORTION);
    pass
}

fn captured_rows(storage: &Path) -> BTreeMap<u32, Vec<String>> {
    let reader = Reader::open(storage).expect("reader");
    let listing = reader.segments(..).expect("listing");
    assert!(listing.warnings.is_empty());
    let mut result = BTreeMap::<u32, Vec<String>>::new();
    for unit in &listing.segments {
        let segment = reader.open_segment(unit).expect("segment");
        let dictionary = segment.dictionary().expect("dictionary");
        for type_id in [1_206_001, 1_207_001, 1_208_001, 1_209_001, 1_210_001] {
            if segment.rows_of(type_id).is_none() {
                continue;
            }
            for row in segment.rows(type_id).expect("recorded rows") {
                assert_eq!(row.get("ts"), Some(&Cell::Ts(SCAN_TS)));
                let mut resolved = String::new();
                for (name, cell) in row.iter() {
                    match cell {
                        Cell::StrId(id) => {
                            let value = dictionary.resolve(*id).expect("recorded string");
                            let Resolved::Str(bytes) = value else {
                                panic!("small fixture string");
                            };
                            write!(
                                resolved,
                                "{name}={};",
                                std::str::from_utf8(bytes).expect("UTF-8")
                            )
                            .expect("string");
                        }
                        cell => write!(resolved, "{name}={cell:?};").expect("cell"),
                    }
                }
                result.entry(type_id).or_default().push(resolved);
            }
        }
    }
    for rows in result.values_mut() {
        rows.sort();
    }
    result
}

fn assert_counts(rows: &BTreeMap<u32, Vec<String>>) {
    for type_id in [1_206_001, 1_207_001, 1_208_001, 1_209_001] {
        assert_eq!(rows[&type_id].len(), 601, "type {type_id}");
    }
    assert_eq!(rows[&1_210_001].len(), 1200);
    for minor in 0..1200 {
        assert!(rows[&1_210_001].iter().any(|row| {
            row.contains(&format!("minor=U32({minor});"))
                && row.contains(&format!("rbytes=I64({minor});"))
        }));
    }
}

#[test]
fn large_discovery_portions_roundtrip_active_and_sealed_without_global_row_drop() {
    let (temp, proc, sys) = fixture();
    let storage = temp.path().join("storage");
    let config = config(&storage);
    let (owner, mut journal) = open_journal(&storage, JournalConfig::default().max_parts);
    let mut segment = SegmentState::default();
    let start = Instant::now();
    let pass = collect(&proc, &sys, &config, &mut journal, &owner, &mut segment);
    let collect_duration = start.elapsed();
    assert!(journal.parts().len() > 3);
    assert!(pass.written.is_empty());
    let raw_bytes = journal.bytes();
    let before = captured_rows(&storage);
    assert_counts(&before);
    let finished = close_open_segment(&mut journal, &owner, &mut segment, "test").expect("seal");
    let after = captured_rows(&storage);
    assert_eq!(
        before, after,
        "full raw values and dictionary text survive sealing"
    );
    println!(
        "offline_discovery groups=601 io_rows=1200 elapsed_us={} wal_bytes={} zms_bytes={} peak_groups={} peak_io={} peak_strings={}",
        collect_duration.as_micros(),
        raw_bytes,
        std::fs::metadata(finished).expect("ZMS metadata").len(),
        pass.peak_groups,
        pass.peak_io_rows,
        pass.peak_string_bytes
    );
}

#[test]
fn one_scan_crosses_many_segment_rollovers_with_one_timestamp_and_unique_ids() {
    let (temp, proc, sys) = fixture();
    let storage = temp.path().join("storage");
    let config = config(&storage);
    let (owner, mut journal) = open_journal(&storage, 2);
    let mut segment = SegmentState::default();
    let pass = collect(&proc, &sys, &config, &mut journal, &owner, &mut segment);
    assert!(pass.written.len() >= 3, "exercise at least three rollovers");
    if !segment.is_empty() {
        close_open_segment(&mut journal, &owner, &mut segment, "test").expect("last segment");
    }
    let reader = Reader::open(&storage).expect("reader");
    let listing = reader.segments(..).expect("listing");
    let ids = listing
        .segments
        .iter()
        .map(kronika_reader::SegmentRef::id)
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), listing.segments.len());
    assert_counts(&captured_rows(&storage));
    for unit in &listing.segments {
        let opened = reader.open_segment(unit).expect("segment");
        assert_eq!(opened.rows_of(1_021_003), Some(1), "opening metadata");
    }
}

#[test]
fn postgresql_mode_never_opens_discovery_roots_or_appends_os_rows() {
    let temp = tempfile::tempdir().expect("fixture");
    let storage = temp.path().join("storage");
    let config = config(&storage);
    let (owner, mut journal) = open_journal(&storage, JournalConfig::default().max_parts);
    let mut segment = SegmentState::default();
    let mut sched = Scheduler::for_mode(Intervals::default(), false);
    let pass = run(
        &ProcFs::new(temp.path().join("missing-proc")),
        &SysFs::new(temp.path().join("missing-sys")),
        &config,
        true,
        &mut journal,
        &owner,
        &mut segment,
        &mut sched,
        SCAN_TS,
        &[],
    )
    .expect("PG-only does not need OS roots");
    assert!(!pass.appended);
    assert!(pass.written.is_empty());
    assert!(journal.parts().is_empty());
    assert_eq!(pass.stats.groups, 0);
}

#[test]
fn unavailable_finite_and_unlimited_limits_remain_distinct() {
    assert_eq!(finite_limit(None), (None, None));
    assert_eq!(finite_limit(Some(-2)), (None, None));
    assert_eq!(finite_limit(Some(-1)), (None, Some(true)));
    assert_eq!(finite_limit(Some(0)), (Some(0), Some(false)));
    assert_eq!(finite_limit(Some(42)), (Some(42), Some(false)));
}
