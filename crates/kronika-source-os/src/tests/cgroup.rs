use super::*;

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

#[test]
fn pids_values_require_valid_current_and_limit() {
    for current in ["", "invalid", "-1"] {
        assert_eq!(parse_pids_values(current, "128"), None);
    }
    for max in ["", "invalid", "-1"] {
        assert_eq!(parse_pids_values("9", max), None);
    }
    assert_eq!(parse_pids_values("0", "max"), Some((0, None)));
    assert_eq!(parse_pids_values("9", "128"), Some((9, Some(128))));
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
fn selected_cgroup_collectors_reject_invalid_unified_memberships() {
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
        assert_eq!(parse_unified_cgroup_path(membership), None);
    }
}

#[test]
fn readable_v1_controllers_do_not_produce_cgroup_metrics_or_capacity() {
    let (dir, procfs, sys) = fixture_roots();
    let membership = "2:cpu,cpuacct:/workload\n3:memory:/workload\n4:blkio:/workload\n5:pids:/workload\n6:cpuset:/workload\n";
    std::fs::write(dir.path().join("proc/self/cgroup"), membership).expect("write membership");
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
    std::fs::write(
        dir.path().join("proc/self/mountinfo"),
        format!(
            "40 1 0:30 / {} rw - cgroup cgroup rw,cpu,cpuacct\n",
            dir.path().join("sys/fs/cgroup").display()
        ),
    )
    .expect("write v1 mount");
    let discovery = discovery::walk_visible_v2(&procfs, &sys, 7, |_| {
        panic!("v1 mount must not produce discovered v2 rows")
    })
    .expect("v1 discovery");
    assert_eq!(discovery.groups, 0);
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
fn hybrid_memberships_select_only_unified_v2_paths() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/workload");
    let membership = "2:cpu,cpuacct:/unrelated\n3:memory:/unrelated\n0::/workload\n";
    std::fs::write(dir.path().join("proc/self/cgroup"), membership)
        .expect("write hybrid membership");
    let selected = collect_ancestor_context(&procfs, &sys, 7).expect("context");
    let context = &selected.context;
    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpu_path.as_deref(), Some("/"));
}
