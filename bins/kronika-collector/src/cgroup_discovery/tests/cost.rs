use super::*;

const SAMPLES: usize = 120;
const LEAVES: usize = 512;
const GROUPS: usize = 1027;
const DEVICES: usize = 1024;
const CHILD_ENV: &str = "KRONIKA_DISCOVERY_COST_CHILD";
const TEST_NAME: &str =
    "cgroup_discovery::tests::cost::discovery_hour_cost_for_prior_physical_population";

fn cost_fixture(root: &Path) -> (ProcFs, SysFs) {
    let proc = root.join("proc");
    let sys = root.join("sys");
    let cgroup = sys.join("fs/cgroup");
    std::fs::create_dir_all(proc.join("self")).expect("proc fixture");
    std::fs::create_dir_all(&cgroup).expect("cgroup fixture");
    std::fs::write(
        proc.join("self/mountinfo"),
        format!("40 1 0:30 / {} rw - cgroup2 cgroup rw\n", cgroup.display()),
    )
    .expect("mountinfo");
    std::fs::write(cgroup.join("cgroup.controllers"), "cpu memory io pids\n").expect("controllers");
    for candidate in 0..LEAVES {
        let leaf = cgroup.join(format!(
            "kubepods.slice/kubepods-burstable.slice/pod-{candidate:04}/container-{candidate:04}"
        ));
        std::fs::create_dir_all(&leaf).expect("leaf");
        for (name, text) in [
            (
                "cpu.stat",
                "usage_usec 100\nuser_usec 60\nsystem_usec 30\nnr_throttled 2\nthrottled_usec 5\n",
            ),
            ("cpu.max", "200000 100000\n"),
            ("memory.current", "536870912\n"),
            ("memory.max", "1073741824\n"),
            (
                "memory.stat",
                "anon 268435456\nfile 134217728\nkernel 67108864\nslab 33554432\n",
            ),
            ("memory.events", "low 0\nhigh 1\nmax 2\noom 0\noom_kill 0\n"),
            ("pids.current", "16\n"),
            ("pids.max", "256\n"),
        ] {
            std::fs::write(leaf.join(name), text).expect("resource fixture");
        }
        let mut io = String::new();
        for device in 0..2 {
            writeln!(
                io,
                "8:{} rbytes=1000 wbytes=2000 rios=10 wios=20",
                candidate * 2 + device
            )
            .expect("device line");
        }
        std::fs::write(leaf.join("io.stat"), io).expect("I/O fixture");
    }
    (ProcFs::new(proc), SysFs::new(sys))
}

fn cpu_ticks() -> i64 {
    let content = std::fs::read_to_string("/proc/self/stat").expect("test process CPU accounting");
    let stat =
        kronika_source_os::proc::process::parse_stat(&content).expect("process CPU counters");
    stat.utime.saturating_add(stat.stime)
}

struct Interval {
    started: Instant,
    cpu: i64,
    peak_rss: Option<u64>,
}

impl Interval {
    fn start() -> Self {
        Self {
            cpu: cpu_ticks(),
            peak_rss: peak_rss_kib(),
            started: Instant::now(),
        }
    }

    fn finish(self, label: &str) -> u64 {
        let elapsed = self.started.elapsed().as_micros();
        let peak_rss = peak_rss_kib().expect("measure process peak RSS");
        println!(
            "offline_discovery_interval phase={label} elapsed_us={elapsed} cpu_ticks={} ticks_per_second={} process_peak_rss_kib_start={:?} process_peak_rss_kib_end={:?}",
            cpu_ticks().saturating_sub(self.cpu),
            rustix::param::clock_ticks_per_second(),
            self.peak_rss,
            Some(peak_rss)
        );
        peak_rss
    }
}

fn acquisition(proc: &ProcFs, sys: &SysFs) {
    let interval = Interval::start();
    for sample in 0..SAMPLES {
        let ts = SCAN_TS + i64::try_from(sample).expect("sample") * 30_000_000;
        let stats = walk_visible_v2(proc, sys, ts, |_| Ok(())).expect("discovery");
        assert_eq!(stats.groups, GROUPS);
        assert_eq!(stats.io_rows, DEVICES);
        assert_eq!(stats.metric_files_read, LEAVES * 9);
        assert_eq!(stats.metric_errors, 0);
        assert_eq!(stats.skipped_directories, 0);
    }
    interval.finish("acquisition_only");
}

fn append_hour(proc: &ProcFs, sys: &SysFs, storage: &Path) {
    let config = config(storage);
    let (owner, mut journal) = open_journal(storage, JournalConfig::default().max_parts);
    let mut segment = SegmentState::default();
    let mut sched = Scheduler::new(Intervals::default());
    let mut pass = CgroupPass::default();
    let mut raw_bytes = 0_u64;
    let interval = Interval::start();
    for sample in 0..SAMPLES {
        let ts = SCAN_TS + i64::try_from(sample).expect("sample") * 30_000_000;
        let mut appender = Appender {
            config: &config,
            in_container: false,
            journal: &mut journal,
            owner: &owner,
            segment: &mut segment,
            sched: &mut sched,
            opening_settings: &[],
            previous_open_ts: None,
            portion: Portion::default(),
        };
        walk_visible_v2(proc, sys, ts, |row| {
            appender
                .accept(&row, false, &mut pass)
                .map_err(io::Error::other)
        })
        .expect("source and writer");
        appender.flush(&mut pass).expect("last portion");
        assert_eq!(
            pass.written.len(),
            sample / 60,
            "measurement counts all raw WAL bytes before controlled sealing"
        );
        if sample % 60 == 59 && !segment.is_empty() {
            raw_bytes += u64::try_from(journal.bytes()).expect("journal bytes fit u64");
            pass.written.push(
                close_open_segment(&mut journal, &owner, &mut segment, "cost").expect("seal"),
            );
        }
    }
    let writer_peak_rss = interval.finish("acquisition_and_production_append_and_seal");
    let reader = Reader::open(storage).expect("reader");
    let listing = reader.segments(..).expect("segments");
    let mut counts = BTreeMap::<u32, usize>::new();
    let mut section_bytes = BTreeMap::<u32, u64>::new();
    let mut zms_bytes = 0_u64;
    for unit in &listing.segments {
        let opened = reader.open_segment(unit).expect("segment");
        zms_bytes += opened.captured_bytes();
        for type_id in [1_206_001, 1_207_001, 1_208_001, 1_209_001, 1_210_001] {
            *counts.entry(type_id).or_default() +=
                opened.rows(type_id).expect("resource rows").len();
            *section_bytes.entry(type_id).or_default() += opened
                .sections()
                .find(|(id, _)| *id == type_id)
                .expect("resource section")
                .1
                .bytes;
        }
    }
    for type_id in [1_206_001, 1_207_001, 1_208_001, 1_209_001] {
        assert_eq!(counts[&type_id], GROUPS * SAMPLES);
    }
    assert_eq!(counts[&1_210_001], DEVICES * SAMPLES);
    assert!(raw_bytes < 64 * 1024 * 1024);
    assert!(zms_bytes < 16 * 1024 * 1024);
    println!(
        "offline_discovery_storage physical_leaves={LEAVES} discovered_groups={GROUPS} devices={DEVICES} snapshots={SAMPLES} segments={} final_open_journal_bytes_sum={raw_bytes} zms_bytes={zms_bytes} per_section_rows={counts:?} per_section_bytes={section_bytes:?} peak_groups={} peak_io_rows={} peak_string_bytes={}",
        listing.segments.len(),
        pass.peak_groups,
        pass.peak_io_rows,
        pass.peak_string_bytes
    );
    assert!(writer_peak_rss > 0);
}

#[test]
fn discovery_hour_cost_for_prior_physical_population() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, "1")
            .output()
            .expect("isolated measurement child");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "measurement child failed");
        return;
    }
    let temp = tempfile::tempdir().expect("ordinary fixture directory");
    let (proc, sys) = cost_fixture(temp.path());
    acquisition(&proc, &sys);
    append_hour(&proc, &sys, &temp.path().join("storage"));
}
