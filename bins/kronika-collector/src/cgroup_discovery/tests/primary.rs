use super::*;

use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::process::Command;
use std::sync::Arc;

use kronika_index::{FindingKind, SeriesBlock};
use kronika_query::{
    DataRequest, FinishedDataset, HourPart, HourRequest, QueryContext, QueryRequest, QuerySink,
    SegmentRequest, Window, execute,
};
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_store::PosixSource;
use serde_json::Value;

const CHILD_ROOT: &str = "KRONIKA_CGROUP_PRIMARY_TEST_ROOT";

fn permissions(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("fixture permissions");
}

fn isolated(test_name: &str, denied: bool) {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        scenario(Path::new(&root), denied);
        return;
    }
    let temp = tempfile::tempdir().expect("isolated process fixture");
    permissions(temp.path(), 0o777);
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_ROOT, temp.path())
        .env("KRONIKA_PROC_ROOT", temp.path().join("proc"))
        .env("KRONIKA_SYS_ROOT", temp.path().join("sys"));
    if denied && rustix::process::geteuid().is_root() {
        command.uid(4242).gid(4242);
    }
    let output = command.output().expect("isolated collector regression");
    eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "isolated production test failed");
}

fn write(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    std::fs::write(path, value).expect("fixture file");
}

fn prepare(root: &Path, denied: bool) {
    write(
        root,
        "proc/stat",
        "cpu  10 0 10 80 0 0 0 0 0 0\ncpu0  10 0 10 80 0 0 0 0 0 0\nbtime 1700000000\n",
    );
    write(root, "proc/sys/kernel/hostname", "cgroup-fixture\n");
    write(root, "proc/sys/kernel/osrelease", "6.12.0\n");
    write(
        root,
        "proc/sys/kernel/random/boot_id",
        "00000000-0000-0000-0000-000000000001\n",
    );
    write(
        root,
        "proc/self/cgroup",
        if denied { "0::/work\n" } else { "0::/\n" },
    );
    write(
        root,
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n",
            root.join("sys/fs/cgroup").display()
        ),
    );
    write(root, "sys/fs/cgroup/cpu.max", "150000 100000\n");
    write(root, "sys/fs/cgroup/memory.max", "1000000\n");
}

fn sample(root: &Path, denied: bool, observation: i64, devices: u32) {
    let group = if denied {
        "sys/fs/cgroup/work"
    } else {
        "sys/fs/cgroup"
    };
    let contents = [
        (
            "cpu.stat",
            format!(
                "usage_usec {}\nuser_usec {}\nsystem_usec 0\nnr_periods 20\nnr_throttled 1\nthrottled_usec 10\n",
                observation * 1_000_000,
                observation * 1_000_000
            ),
        ),
        (
            "cpu.max",
            if denied {
                "max 100000\n".to_owned()
            } else {
                "150000 100000\n".to_owned()
            },
        ),
        ("cpuset.cpus.effective", "0-3\n".to_owned()),
        ("memory.current", format!("{}\n", observation * 100)),
        (
            "memory.max",
            if denied {
                "2000000\n".to_owned()
            } else {
                "1000000\n".to_owned()
            },
        ),
        (
            "memory.stat",
            "anon 10\nfile 20\nkernel 30\nslab 4\n".to_owned(),
        ),
        (
            "memory.events",
            format!(
                "low 0\nhigh 0\nmax 0\noom {}\noom_kill {}\n",
                observation - 1,
                observation - 1
            ),
        ),
        (
            "memory.events.local",
            format!(
                "high 0\nmax 0\noom {}\noom_kill {}\noom_group_kill 0\n",
                observation - 1,
                observation - 1
            ),
        ),
        ("pids.current", "3\n".to_owned()),
        ("pids.max", "32\n".to_owned()),
        ("pids.events.local", "max 0\n".to_owned()),
    ];
    for (name, content) in contents {
        write(root, &format!("{group}/{name}"), &content);
    }
    for resource in ["cpu", "memory", "io"] {
        write(
            root,
            &format!("{group}/{resource}.pressure"),
            &format!(
                "some avg10=0.00 avg60=0.00 avg300=0.00 total={}\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
                observation * 100
            ),
        );
    }
    let mut io = String::new();
    for minor in 0..devices {
        writeln!(
            io,
            "8:{minor} rbytes={} wbytes=0 rios=0 wios=0",
            observation * 100
        )
        .expect("I/O row");
    }
    write(root, &format!("{group}/io.stat"), &io);
}

fn scenario(root: &Path, denied: bool) {
    prepare(root, denied);
    let storage = root.join("storage");
    let mut config = config(&storage);
    config.mode = CollectorMode::Local;
    let (owner, mut journal) = open_journal(&storage, 2);
    let mut state = SegmentState::default();
    let mut sched = Scheduler::new(Intervals::default());
    let fs = ProcFs::from_env();
    let sys = SysFs::from_env();
    let first = crate::collection_timestamp().expect("clock");
    let devices = if denied { 2 } else { 1100 };
    let mut selected_identity = None;
    let mut written = 0;
    for observation in 1..=2 {
        sample(root, denied, observation, devices);
        if denied {
            deny_parent(root);
        }
        let ts = first + (observation - 1) * 30_000_000;
        let pass = run(
            &fs,
            &sys,
            &config,
            true,
            &mut journal,
            &owner,
            &mut state,
            &mut sched,
            ts,
            &[],
        )
        .expect("production container discovery");
        let selected = pass
            .selected
            .group
            .as_ref()
            .expect("selected readable group");
        assert_eq!(selected.path, if denied { "/work" } else { "/" });
        if let Some(identity) = &selected_identity {
            assert_eq!(identity, &selected.identity);
        }
        selected_identity = Some(selected.identity.clone());
        assert_eq!(
            pass.selected.context.effective_cpu_quota_usec,
            Some(150_000)
        );
        assert_eq!(pass.selected.context.effective_memory_max, Some(1_000_000));
        assert_eq!(pass.stats.groups, 1);
        assert_eq!(pass.stats.io_rows, devices as usize);
        assert_eq!(pass.charged_devices.len(), devices as usize);
        if denied {
            assert!(pass.stats.skipped_directories > 0);
            permissions(&root.join("sys/fs/cgroup"), 0o755);
        }
        written += pass.written.len();
        check_sources(&storage, &selected.identity);
        normal_os(
            &mut journal,
            &owner,
            &config,
            &mut state,
            &mut sched,
            ts,
            &pass,
        );
        check_sources(&storage, &selected.identity);
    }
    if !denied {
        assert!(
            written >= 3,
            "rejected append retries cross multiple segments"
        );
    }
    if !state.is_empty() {
        close_open_segment(&mut journal, &owner, &mut state, "test").expect("final seal");
    }
    let identity = selected_identity.expect("selected identity");
    check_sources(&storage, &identity);
    check_consumers(&storage, first, first + 30_000_000, devices);
    eprintln!(
        "primary_context_fixture=EXECUTED uid={} denied_parent={} observations=2 devices={} closed_segments={written}",
        rustix::process::geteuid().as_raw(),
        denied,
        devices
    );
}

fn deny_parent(root: &Path) {
    let group = root.join("sys/fs/cgroup");
    permissions(&group, 0o111);
    assert!(
        !rustix::process::geteuid().is_root(),
        "permission case needs unprivileged execution"
    );
    assert!(
        std::fs::read_dir(&group).is_err(),
        "parent cannot be enumerated"
    );
    assert!(std::fs::read_to_string(group.join("work/cpu.stat")).is_ok());
    eprintln!(
        "permission_fixture=EXECUTED uid={} root_listing=denied known_child=readable",
        rustix::process::geteuid().as_raw()
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "exercise the normal collector append with the same discovery pass"
)]
fn normal_os(
    journal: &mut Journal,
    owner: &WriterOwner,
    config: &Config,
    state: &mut SegmentState,
    sched: &mut Scheduler,
    ts: i64,
    pass: &CgroupPass,
) {
    let mut credentials = Some(ProcessIoCredentials::new());
    let outcome = crate::append_pending_window(
        journal,
        owner,
        config,
        true,
        &crate::scheduler::DueSet::for_test(vec![crate::scheduler::SourceKind::OsCore]),
        &crate::LogRows::default(),
        &[],
        ts,
        &mut credentials,
        state,
        sched,
        Some(pass),
    )
    .expect("normal OS follow-up append");
    assert!(outcome.accepted && outcome.appended);
}

fn resolved(segment: &kronika_reader::Segment, row: &kronika_reader::Row, field: &str) -> String {
    let Some(Cell::StrId(id)) = row.get(field) else {
        panic!("required dictionary identity {field}");
    };
    let dictionary = segment.dictionary().expect("dictionary");
    String::from_utf8(
        dictionary
            .resolve(*id)
            .expect("identity bytes")
            .stored_bytes()
            .to_vec(),
    )
    .expect("identity text")
}

fn check_sources(storage: &Path, expected_identity: &str) {
    let reader = Reader::open(storage).expect("reader");
    for unit in reader.segments(..).expect("sources").segments {
        let segment = reader.open_segment(&unit).expect("source");
        let context = segment
            .rows(1_205_002)
            .expect("primary context in each contributing source");
        let mut stamps = HashSet::new();
        for row in &context {
            assert!(
                stamps.insert(format!("{:?}", row.get("ts"))),
                "duplicate same-time context in segment {}",
                unit.id()
            );
            assert_eq!(resolved(&segment, row, "cpu_identity"), expected_identity);
            assert_eq!(
                resolved(&segment, row, "memory_identity"),
                expected_identity
            );
            assert_eq!(resolved(&segment, row, "io_identity"), expected_identity);
        }
        for type_id in [1_201_003, 1_202_003, 1_203_003, 1_204_001] {
            if segment.rows_of(type_id).is_none() {
                continue;
            }
            for row in segment.rows(type_id).expect("primary resource rows") {
                assert!(
                    context.iter().any(|ctx| ctx.get("ts") == row.get("ts")),
                    "primary row without same-observation context"
                );
                if type_id != 1_204_001 {
                    assert_eq!(
                        resolved(&segment, &row, "cgroup_identity"),
                        expected_identity
                    );
                }
                assert!(!resolved(&segment, &row, "cgroup_path").is_empty());
            }
        }
    }
}

#[derive(Default)]
struct Records(Vec<Value>);
impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0
            .push(serde_json::from_slice(&bytes).expect("query JSON"));
        true
    }
    fn cancelled(&self) -> bool {
        false
    }
}

fn check_consumers(storage: &Path, first: i64, second: i64, devices: u32) {
    let reader = Reader::open(storage).expect("reader");
    let mut oom = 0;
    let segment_ids: Vec<_> = reader
        .segments(..)
        .expect("sources")
        .segments
        .iter()
        .map(kronika_reader::SegmentRef::id)
        .collect();
    let context = QueryContext::new(
        Arc::new(FinishedDataset::new(
            PosixSource::open(storage).expect("source"),
        )),
        1,
        false,
    );
    let mut history_counts = BTreeMap::<u32, usize>::new();
    for unit in reader.segments(..).expect("segments").segments {
        let segment = reader.open_segment(&unit).expect("segment");
        let index = kronika_index::build_from_reader(&reader, &unit, &segment)
            .expect("actual index with predecessors");
        for block in index.blocks {
            if let SeriesBlock::Findings(block) = block
                && block.type_id == 1_202_003
            {
                oom += block
                    .findings
                    .iter()
                    .filter(|finding| {
                        finding.kind == FindingKind::KnownBad && finding.timestamp == second
                    })
                    .count();
            }
        }
        for (type_id, section, field) in [
            (1_201_003, "os_cgroup_cpu", "usage_usec"),
            (1_202_003, "os_cgroup_memory", "oom_kill"),
            (1_203_003, "os_cgroup_io", "rbytes"),
        ] {
            if segment.rows_of(type_id).is_none() {
                continue;
            }
            let mut records = Records::default();
            execute(
                &context,
                QueryRequest::History(DataRequest {
                    segment: SegmentRequest {
                        segment_id: unit.id(),
                        section: section.to_owned(),
                    },
                    fields: vec![field.to_owned()],
                    filters: Vec::new(),
                    type_id: Some(type_id),
                    after: None,
                }),
            )
            .expect("selected history")
            .stream(&mut records)
            .expect("history stream");
            *history_counts.entry(type_id).or_default() += records
                .0
                .iter()
                .filter(|row| row["record"] == "row")
                .count();
        }
    }
    assert_eq!(oom, 1, "actual selected OOM finding survives split storage");
    assert_eq!(history_counts[&1_201_003], 2);
    assert_eq!(history_counts[&1_202_003], 2);
    assert_eq!(history_counts[&1_203_003], devices as usize * 2);
    eprintln!(
        "primary_consumers=PASSED index_oom_findings={oom} cpu_history={} memory_history={} io_history={}",
        history_counts[&1_201_003], history_counts[&1_202_003], history_counts[&1_203_003]
    );
    check_lanes(&context, segment_ids, first, second, devices);
}

fn check_lanes(
    context: &QueryContext,
    segment_ids: Vec<i64>,
    first: i64,
    second: i64,
    devices: u32,
) {
    let mut lanes = Records::default();
    execute(
        context,
        QueryRequest::Hour(HourRequest {
            window: Window {
                from: Some(first),
                to: Some(second),
            },
            series: None,
            part: HourPart::Lanes,
            segments: Some(segment_ids),
            active: None,
        }),
    )
    .expect("selected lanes")
    .stream(&mut lanes)
    .expect("lane stream");
    for (key, expected) in [
        ("cg_cpu_cores", 1.0 / 30.0),
        ("cg_cpu_share", 100.0 / 45.0),
        ("cg_io_read", f64::from(devices) * 100.0 / 30.0),
    ] {
        let at_second: Vec<_> = lanes
            .0
            .iter()
            .filter(|row| {
                row["record"] == "lane"
                    && row["lane"] == key
                    && row["ts"]
                        .as_str()
                        .and_then(|value| value.parse::<i64>().ok())
                        == Some(second)
            })
            .collect();
        assert_eq!(at_second.len(), 1, "one complete {key} observation");
        assert!(
            at_second[0]["value"].is_number(),
            "{key}: selected rate is unknown; points={at_second:?}"
        );
        let actual = at_second[0]["value"]
            .as_f64()
            .expect("selected resource rate");
        assert!(
            (actual - expected).abs() < 0.000_001,
            "{key}: actual={actual} expected={expected}"
        );
    }
}

#[test]
fn container_primary_context_survives_rollover_retry_and_normal_os_followup() {
    isolated(
        "cgroup_discovery::tests::primary::container_primary_context_survives_rollover_retry_and_normal_os_followup",
        false,
    );
}

#[test]
fn nonenumerable_parent_preserves_all_selected_resources_under_unprivileged_reader() {
    isolated(
        "cgroup_discovery::tests::primary::nonenumerable_parent_preserves_all_selected_resources_under_unprivileged_reader",
        true,
    );
}
