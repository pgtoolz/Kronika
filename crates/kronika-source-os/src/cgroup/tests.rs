use super::*;
use std::fmt::Write as _;

fn collect_workloads(procfs: &ProcFs, sys: &SysFs, ts: i64) -> io::Result<CgroupCollection> {
    let mut memberships = WorkloadMemberships::new(sys);
    for pid in procfs.pid_dirs()? {
        let Ok(content) = procfs.read_raw(&format!("{pid}/cgroup")) else {
            // Processes can exit between enumerating /proc and reading their
            // membership. The remaining live snapshot is still coherent.
            continue;
        };
        memberships.observe(&content);
    }
    if let Ok(content) = procfs.read_raw("self/cgroup") {
        memberships.observe(&content);
    }
    memberships.collect(sys, ts)
}

fn fixture_roots() -> (tempfile::TempDir, ProcFs, SysFs) {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("self")).expect("mkdir proc self");
    std::fs::create_dir_all(sys_root.join("fs/cgroup")).expect("mkdir cgroup root");
    (dir, ProcFs::new(proc_root), SysFs::new(sys_root))
}

fn fixture_cgroup_path(
    dir: &tempfile::TempDir,
    controller: &str,
    path: &str,
) -> std::path::PathBuf {
    let mut root = dir.path().join("sys/fs/cgroup");
    if !controller.is_empty() {
        root.push(controller);
    }
    root.join(path.trim_start_matches('/'))
}

fn prepare_v2_context(dir: &tempfile::TempDir, path: &str) {
    std::fs::write(
        dir.path().join("proc/self/mountinfo"),
        format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n",
            dir.path().join("sys/fs/cgroup").display()
        ),
    )
    .expect("write mount binding");
    std::fs::write(dir.path().join("proc/self/cgroup"), format!("0::{path}\n"))
        .expect("write v2 membership");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io cpuset\n",
    )
    .expect("write v2 controllers");
    let leaf = fixture_cgroup_path(dir, "", path);
    std::fs::create_dir_all(&leaf).expect("mkdir v2 leaf");
    std::fs::write(
        leaf.join("cpu.stat"),
        "usage_usec 10\nuser_usec 6\nsystem_usec 4\n",
    )
    .expect("write v2 cpu stat");
    std::fs::write(leaf.join("memory.current"), "4096\n").expect("write v2 memory current");
    std::fs::write(
        leaf.join("memory.stat"),
        "anon 100\nfile 200\nkernel 50\nslab 20\n",
    )
    .expect("write v2 memory stat");
    std::fs::write(
        leaf.join("io.stat"),
        "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n",
    )
    .expect("write v2 io stat");
    std::fs::write(leaf.join("cpuset.cpus.effective"), "0-1\n").expect("write v2 effective cpuset");
}

fn write_process_membership(dir: &tempfile::TempDir, pid: i32, content: &str) {
    let process = dir.path().join("proc").join(pid.to_string());
    std::fs::create_dir_all(&process).expect("mkdir process");
    std::fs::write(process.join("cgroup"), content).expect("write process cgroup membership");
}

fn write_v2_workload_files(dir: &tempfile::TempDir, path: &str, io_stat: &str) {
    let workload = fixture_cgroup_path(dir, "", path);
    std::fs::create_dir_all(&workload).expect("mkdir v2 workload");
    std::fs::write(
        workload.join("cpu.stat"),
        "usage_usec 100\nuser_usec 60\nsystem_usec 40\n",
    )
    .expect("write workload cpu.stat");
    std::fs::write(workload.join("memory.current"), "4096\n")
        .expect("write workload memory.current");
    std::fs::write(workload.join("pids.current"), "4\n").expect("write workload pids.current");
    std::fs::write(workload.join("pids.max"), "max\n").expect("write workload pids.max");
    std::fs::write(workload.join("io.stat"), io_stat).expect("write workload io.stat");
}

fn write_optional_file(path: &std::path::Path, content: Option<&str>) {
    if let Some(content) = content {
        std::fs::write(path, content).expect("write optional cgroup fixture");
    }
}

fn collect_v2_pids_fixture(current: Option<&str>, max: Option<&str>) -> CgroupCollection {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let workload = root.join("workload");
    std::fs::create_dir_all(&workload).expect("mkdir v2 pids fixture");
    std::fs::write(root.join("cgroup.controllers"), "pids\n").expect("write v2 controllers");
    write_optional_file(&workload.join("pids.current"), current);
    write_optional_file(&workload.join("pids.max"), max);

    let sys = SysFs::new(dir.path().to_path_buf());
    let mut memberships = WorkloadMemberships::new(&sys);
    memberships.observe("0::/workload\n");
    memberships
        .collect(&sys, 7)
        .expect("collect direct workload")
}

#[test]
fn workload_collection_uses_only_direct_live_v2_memberships() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io pids\n",
    )
    .expect("write controllers");
    write_process_membership(&dir, 101, "0::/team/alpha\n");
    write_process_membership(&dir, 102, "0::/team/alpha\n");
    write_process_membership(&dir, 201, "0::/team/beta\n");
    write_v2_workload_files(&dir, "/team/alpha", "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n");
    write_v2_workload_files(&dir, "/team/beta", "8:1 rbytes=5 wbytes=6 rios=7 wios=8\n");
    write_v2_workload_files(
        &dir,
        "/team/unoccupied",
        "8:2 rbytes=9 wbytes=10 rios=11 wios=12\n",
    );

    let rows = collect_workloads(&procfs, &sys, 7).expect("collect workloads");

    assert_eq!(
        rows.cpu
            .iter()
            .map(|row| row.cgroup_path.as_str())
            .collect::<Vec<_>>(),
        ["/team/alpha", "/team/beta"]
    );
    assert_eq!(rows.memory.len(), 2);
    assert_eq!(rows.pids.len(), 2);
    assert_eq!(rows.io.len(), 2);
    assert!(!rows.io_omitted);
}

#[test]
fn workload_candidate_count_overflow_rejects_the_complete_tick() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io pids\n",
    )
    .expect("write controllers");
    for pid in 1..=MAX_CGROUP_CANDIDATES + 1 {
        write_process_membership(
            &dir,
            i32::try_from(pid).expect("bounded PID fixture"),
            &format!("0::/workload/{pid}\n"),
        );
    }

    let err = collect_workloads(&procfs, &sys, 7).expect_err("candidate limit");

    assert!(err.to_string().contains("membership count exceeds 512"));
}

#[test]
fn workload_path_bytes_overflow_rejects_the_complete_tick() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io pids\n",
    )
    .expect("write controllers");
    let padding = "x".repeat((MAX_CGROUP_PATH_BYTES / MAX_CGROUP_CANDIDATES) + 1);
    for pid in 1..=MAX_CGROUP_CANDIDATES {
        write_process_membership(
            &dir,
            i32::try_from(pid).expect("bounded PID fixture"),
            &format!("0::/{padding}/{pid:04}\n"),
        );
    }

    let err = collect_workloads(&procfs, &sys, 7).expect_err("path byte limit");

    assert!(
        err.to_string()
            .contains("membership paths exceed 524288 bytes")
    );
}

#[test]
fn workload_io_overflow_omits_only_the_complete_io_section() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io pids\n",
    )
    .expect("write controllers");
    write_process_membership(&dir, 101, "0::/workload\n");
    let mut io_stat = String::new();
    for minor in 0..=MAX_CGROUP_IO_ROWS {
        writeln!(io_stat, "8:{minor} rbytes=1 wbytes=2 rios=3 wios=4").expect("write I/O fixture");
    }
    write_v2_workload_files(&dir, "/workload", &io_stat);

    let rows = collect_workloads(&procfs, &sys, 7).expect("collect workloads");

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.pids.len(), 1);
    assert!(rows.io.is_empty());
    assert!(rows.io_omitted);
}

#[test]
fn v2_pids_omits_rows_without_a_valid_current_value() {
    for current in [None, Some("invalid\n"), Some("-1\n")] {
        let rows = collect_v2_pids_fixture(current, Some("128\n"));
        assert!(rows.pids.is_empty(), "current={current:?}");
    }
}

#[test]
fn v2_pids_omits_rows_without_a_valid_max_value() {
    for max in [None, Some("invalid\n"), Some("-1\n")] {
        let rows = collect_v2_pids_fixture(Some("9\n"), max);
        assert!(rows.pids.is_empty(), "max={max:?}");
    }
}

#[test]
fn direct_v2_workload_reads_every_controller_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let workload = root.join("workload");
    std::fs::create_dir_all(&workload).expect("mkdir cgroup");
    std::fs::write(root.join("cgroup.controllers"), "cpu memory io pids\n")
        .expect("write controllers");
    std::fs::write(
        workload.join("cpu.stat"),
        "usage_usec 100\nuser_usec 60\nsystem_usec 40\nnr_throttled 2\nthrottled_usec 500\n",
    )
    .expect("write cpu.stat");
    std::fs::write(workload.join("cpu.max"), "200000 100000\n").expect("write cpu.max");
    std::fs::write(workload.join("memory.current"), "4096\n").expect("write memory.current");
    std::fs::write(workload.join("memory.max"), "max\n").expect("write memory.max");
    std::fs::write(
        workload.join("memory.stat"),
        "anon 100\nfile 200\nkernel 50\nslab 20\n",
    )
    .expect("write memory.stat");
    std::fs::write(
        workload.join("memory.events"),
        "low 1\nhigh 2\nmax 3\noom 4\noom_kill 5\n",
    )
    .expect("write memory.events");
    std::fs::write(workload.join("pids.current"), "7\n").expect("write pids.current");
    std::fs::write(workload.join("pids.max"), "max\n").expect("write pids.max");
    std::fs::write(
        workload.join("io.stat"),
        "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n\
             259:0 rbytes=5 wbytes=6 rios=7 wios=8\n",
    )
    .expect("write io.stat");

    let sys = SysFs::new(dir.path().to_path_buf());
    let mut memberships = WorkloadMemberships::new(&sys);
    memberships.observe("0::/workload\n");
    let rows = memberships
        .collect(&sys, 99)
        .expect("collect direct workload");

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.io.len(), 2);
    assert_eq!(rows.pids.len(), 1);

    let cpu = &rows.cpu[0];
    assert_eq!(cpu.cgroup_path, "/workload");
    assert_eq!(cpu.ts, 99);
    assert_eq!(cpu.usage_usec, 100);
    assert_eq!(cpu.user_usec, 60);
    assert_eq!(cpu.system_usec, 40);
    assert_eq!(cpu.nr_throttled, 2);
    assert_eq!(cpu.throttled_usec, 500);
    assert_eq!(cpu.quota_usec, 200_000);
    assert_eq!(cpu.period_usec, 100_000);

    let memory = &rows.memory[0];
    assert_eq!(memory.current, 4096);
    assert_eq!(memory.max, None);
    assert_eq!(memory.anon, 100);
    assert_eq!(memory.file, 200);
    assert_eq!(memory.kernel, 50);
    assert_eq!(memory.slab, 20);
    assert_eq!(memory.low_events, 1);
    assert_eq!(memory.high_events, 2);
    assert_eq!(memory.max_events, 3);
    assert_eq!(memory.oom_events, 4);
    assert_eq!(memory.oom_kill, 5);

    assert_eq!(rows.pids[0].current, 7);
    assert_eq!(rows.pids[0].max, None);
    assert_eq!((rows.io[0].major, rows.io[0].minor), (8, 0));
    assert_eq!(rows.io[0].rbytes, Some(1));
    assert_eq!(rows.io[0].wios, Some(4));
}

#[test]
fn section_conversions_preserve_metric_fields() {
    use kronika_registry::{StrId, Ts};

    let cgroup_path = StrId(55);
    let cpu = CgroupCpuRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        usage_usec: 100,
        user_usec: 60,
        system_usec: 40,
        throttled_usec: 5,
        nr_throttled: 2,
        quota_usec: -1,
        period_usec: 100_000,
    };
    let memory = CgroupMemoryRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        current: 4096,
        max: None,
        anon: 100,
        file: 200,
        kernel: 50,
        slab: 20,
        low_events: 1,
        high_events: 2,
        max_events: 3,
        oom_events: 4,
        oom_kill: 5,
    };
    let io = CgroupIoRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        major: 8,
        minor: 0,
        rbytes: Some(1),
        wbytes: Some(2),
        rios: Some(3),
        wios: Some(4),
    };
    let pids = CgroupPidsRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        current: 9,
        max: Some(128),
    };
    let cpu_section = to_cpu_section(&cpu, 2, cgroup_path);
    assert_eq!(cpu_section.ts, Ts(7));
    assert_eq!(cpu_section.cgroup_path, cgroup_path);
    assert_eq!(cpu_section.scope, 2);
    assert_eq!(cpu_section.usage_usec, 100);
    assert_eq!(cpu_section.nr_throttled, 2);

    let memory_section = to_memory_section(&memory, 2, cgroup_path);
    assert_eq!(memory_section.ts, Ts(7));
    assert_eq!(memory_section.max, None);
    assert_eq!(memory_section.oom_kill, 5);

    let io_section = to_io_section(&io, 2, cgroup_path);
    assert_eq!((io_section.major, io_section.minor), (8, 0));
    assert_eq!(io_section.rbytes, Some(1));
    assert_eq!(io_section.wios, Some(4));

    let pids_section = to_pids_section(&pids, 2, cgroup_path);
    assert_eq!(pids_section.current, 9);
    assert_eq!(pids_section.max, Some(128));
}

#[test]
fn all_cgroup_collectors_reject_invalid_unified_memberships() {
    for membership in [
        "7::/workload\n",
        "0::/workload\n0::/workload\n",
        "0::/workload\n0::/other\n0::/workload\n",
        "0::/workload/../outside\n0::/workload\n",
    ] {
        let (dir, procfs, sys) = fixture_roots();
        prepare_v2_context(&dir, "/workload");
        std::fs::write(dir.path().join("proc/self/cgroup"), membership).expect("write membership");
        let selected =
            collect_ancestor_context(&procfs, &sys, 1).expect("invalid membership omitted");
        assert!(selected.group.is_none(), "{membership}");
        assert!(
            collect_ancestor_rows(&sys, &selected, 1)
                .ancestor_cpu
                .is_empty()
        );
        assert!(
            collect_ancestor_pressure(&sys, &selected, 1)
                .expect("no selected PSI")
                .is_empty()
        );
        assert!(charged_ancestor_devices(&sys, &selected).is_empty());
        let mut memberships = WorkloadMemberships::new(&sys);
        memberships.observe(membership);
        let workload = memberships.collect(&sys, 1).expect("collect workload");
        assert!(workload.cpu.is_empty(), "{membership}");
        assert!(workload.memory.is_empty(), "{membership}");
        assert!(workload.io.is_empty(), "{membership}");
        assert!(workload.pids.is_empty(), "{membership}");
    }
}

#[test]
fn readable_v1_controllers_do_not_produce_cgroup_metrics_or_capacity() {
    let (dir, procfs, sys) = fixture_roots();
    let membership = "2:cpu,cpuacct:/workload\n3:memory:/workload\n4:blkio:/workload\n5:pids:/workload\n6:cpuset:/workload\n";
    std::fs::write(dir.path().join("proc/self/cgroup"), membership).expect("write membership");
    write_process_membership(&dir, 42, membership);
    for (controller, files) in [
        (
            "cpu,cpuacct",
            vec![
                ("cpuacct.usage", "200000000"),
                ("cpuacct.stat", "user 30\nsystem 20"),
                ("cpu.cfs_quota_us", "50000"),
                ("cpu.cfs_period_us", "100000"),
            ],
        ),
        (
            "memory",
            vec![
                ("memory.usage_in_bytes", "8192"),
                ("memory.limit_in_bytes", "16384"),
                (
                    "memory.stat",
                    "total_rss 1000\ntotal_cache 2000\ntotal_slab 300\ntotal_kernel_stack 40\nhierarchical_memory_limit 16384",
                ),
            ],
        ),
        (
            "blkio",
            vec![
                (
                    "blkio.throttle.io_service_bytes",
                    "8:0 Read 10\n8:0 Write 20",
                ),
                ("blkio.throttle.io_serviced", "8:0 Read 1\n8:0 Write 2"),
            ],
        ),
        ("pids", vec![("pids.current", "9"), ("pids.max", "128")]),
        ("cpuset", vec![("cpuset.effective_cpus", "0-7")]),
    ] {
        let path = fixture_cgroup_path(&dir, controller, "/workload");
        std::fs::create_dir_all(&path).expect("mkdir controller");
        for (name, value) in files {
            std::fs::write(path.join(name), value).expect("write readable controller");
        }
    }
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cpu.stat"),
        "nr_periods 9\nnr_throttled 3\nthrottled_time 700000000\n",
    )
    .expect("write directly mounted v1 CPU controller");
    for rows in [
        collect_workloads(&procfs, &sys, 7).expect("workloads"),
        collect_workload_memberships([membership], &sys, 7).expect("memberships"),
    ] {
        assert!(rows.cpu.is_empty());
        assert!(rows.memory.is_empty());
        assert!(rows.io.is_empty());
        assert!(rows.pids.is_empty());
    }
    let selected = collect_ancestor_context(&procfs, &sys, 7).expect("context");
    let context = &selected.context;
    assert_eq!(context.cgroup_version, 0);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.memory_path, None);
    assert_eq!(context.io_path, None);
    assert_eq!(context.cpuset_cpus, None);
    assert_eq!(context.effective_cpu_quota_usec, None);
    assert_eq!(context.effective_cpu_period_usec, None);
    assert_eq!(context.effective_memory_max, None);
    assert!(
        collect_ancestor_pressure(&sys, &selected, 7)
            .expect("pressure")
            .is_empty()
    );
    assert!(charged_ancestor_devices(&sys, &selected).is_empty());
    assert_eq!(crate::proc::process::parse_cgroup_path(membership), None);
}

#[test]
fn hybrid_memberships_collect_only_unified_v2_paths() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/workload");
    let membership = "2:cpu,cpuacct:/unrelated\n3:memory:/unrelated\n0::/workload\n";
    std::fs::write(dir.path().join("proc/self/cgroup"), membership)
        .expect("write hybrid membership");
    let rows = collect_workload_memberships([membership], &sys, 7).expect("memberships");
    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.cpu[0].cgroup_path, "/workload");
    let selected = collect_ancestor_context(&procfs, &sys, 7).expect("context");
    let context = &selected.context;
    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpu_path.as_deref(), Some("/"));
}
