use super::*;
use std::fmt::Write as _;

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

fn write_v2_capacity(dir: &tempfile::TempDir, path: &str, cpu_max: &str, memory_max: &str) {
    let cgroup = fixture_cgroup_path(dir, "", path);
    std::fs::create_dir_all(&cgroup).expect("mkdir v2 capacity cgroup");
    std::fs::write(cgroup.join("cpu.max"), cpu_max).expect("write v2 cpu max");
    std::fs::write(cgroup.join("memory.max"), memory_max).expect("write v2 memory max");
}

fn prepare_v1_context(
    dir: &tempfile::TempDir,
    path: &str,
    local_memory_limit: &str,
    hierarchical_memory_limit: &str,
) {
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        format!("2:cpu,cpuacct:{path}\n3:memory:{path}\n4:cpuset:{path}\n"),
    )
    .expect("write v1 membership");
    let cpu = fixture_cgroup_path(dir, "cpu,cpuacct", path);
    let memory = fixture_cgroup_path(dir, "memory", path);
    let cpuset = fixture_cgroup_path(dir, "cpuset", path);
    for cgroup in [&cpu, &memory, &cpuset] {
        std::fs::create_dir_all(cgroup).expect("mkdir v1 leaf");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write v1 CPU usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n")
        .expect("write v1 CPU account stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n")
        .expect("write v1 memory current");
    std::fs::write(memory.join("memory.limit_in_bytes"), local_memory_limit)
        .expect("write v1 local memory limit");
    std::fs::write(
        memory.join("memory.stat"),
        format!(
            "total_rss 100\ntotal_cache 200\ntotal_slab 20\n\
             total_kernel_stack 5\nhierarchical_memory_limit {hierarchical_memory_limit}\n"
        ),
    )
    .expect("write v1 memory stat");
    std::fs::write(cpuset.join("cpuset.effective_cpus"), "0-1\n")
        .expect("write v1 effective cpuset");
}

fn write_v1_cpu_capacity(dir: &tempfile::TempDir, path: &str, quota: &str, period: &str) {
    let cgroup = fixture_cgroup_path(dir, "cpu,cpuacct", path);
    std::fs::create_dir_all(&cgroup).expect("mkdir v1 CPU cgroup");
    std::fs::write(cgroup.join("cpu.cfs_quota_us"), quota).expect("write v1 CPU quota");
    std::fs::write(cgroup.join("cpu.cfs_period_us"), period).expect("write v1 CPU period");
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

const CPU_PRESSURE: &str = "some avg10=0.10 avg60=0.05 avg300=0.02 total=10000\n";
const MEMORY_PRESSURE: &str = "some avg10=1.50 avg60=0.80 avg300=0.30 total=500000\n\
full avg10=0.20 avg60=0.10 avg300=0.05 total=100000\n";
const IO_PRESSURE: &str = "some avg10=0.50 avg60=0.25 avg300=0.10 total=200000\n\
full avg10=0.05 avg60=0.02 avg300=0.01 total=20000\n";

fn prepare_v2_pressure(dir: &tempfile::TempDir, path: &str) -> std::path::PathBuf {
    std::fs::write(dir.path().join("proc/self/cgroup"), format!("0::{path}\n"))
        .expect("write unified membership");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write unified marker");
    let cgroup = fixture_cgroup_path(dir, "", path);
    std::fs::create_dir_all(&cgroup).expect("mkdir pressure cgroup");
    cgroup
}

#[test]
fn pressure_reads_only_the_exact_unified_v2_membership() {
    let (dir, procfs, sys) = fixture_roots();
    let cgroup = prepare_v2_pressure(&dir, "/team/workload");
    std::fs::write(cgroup.join("cpu.pressure"), CPU_PRESSURE).expect("write CPU pressure");
    std::fs::write(cgroup.join("memory.pressure"), MEMORY_PRESSURE).expect("write memory pressure");
    std::fs::write(cgroup.join("io.pressure"), IO_PRESSURE).expect("write I/O pressure");
    let other = fixture_cgroup_path(&dir, "", "/team/other");
    std::fs::create_dir_all(&other).expect("mkdir other cgroup");
    std::fs::write(
        other.join("cpu.pressure"),
        "some avg10=9.00 avg60=9.00 avg300=9.00 total=90000\n",
    )
    .expect("write other pressure");

    let rows = collect_pressure(&procfs, &sys, 77).expect("collect cgroup pressure");

    assert_eq!(
        rows.iter()
            .map(|row| (row.resource, row.ts, row.some_total))
            .collect::<Vec<_>>(),
        [(0, 77, 10_000), (1, 77, 500_000), (2, 77, 200_000)]
    );
}

#[test]
fn pressure_accepts_the_unified_root_membership() {
    let (dir, procfs, sys) = fixture_roots();
    let cgroup = prepare_v2_pressure(&dir, "/");
    std::fs::write(cgroup.join("cpu.pressure"), CPU_PRESSURE).expect("write root pressure");

    let rows = collect_pressure(&procfs, &sys, 88).expect("collect root pressure");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].some_total, 10_000);
}

#[test]
fn pressure_rejects_a_membership_path_that_leaves_its_root() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write unified marker");
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/workload/../outside\n",
    )
    .expect("write unsafe membership");
    let outside = fixture_cgroup_path(&dir, "", "/outside");
    std::fs::create_dir_all(&outside).expect("mkdir outside cgroup");
    std::fs::write(outside.join("cpu.pressure"), CPU_PRESSURE).expect("write outside pressure");

    let error = collect_pressure(&procfs, &sys, 99).expect_err("reject unsafe membership");

    assert!(error.to_string().contains("no single valid unified"));
}

#[test]
fn pressure_rejects_ambiguous_unified_memberships() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write unified marker");
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/first\n0::/second\n",
    )
    .expect("write ambiguous membership");

    let error = collect_pressure(&procfs, &sys, 100).expect_err("reject ambiguous membership");

    assert!(error.to_string().contains("no single valid unified"));
}

#[test]
fn pressure_rejects_a_nonzero_unified_hierarchy_id() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write unified marker");
    let cgroup = fixture_cgroup_path(&dir, "", "/workload");
    std::fs::create_dir_all(&cgroup).expect("mkdir pressure cgroup");
    std::fs::write(cgroup.join("cpu.pressure"), CPU_PRESSURE).expect("write CPU pressure");
    std::fs::write(dir.path().join("proc/self/cgroup"), "7::/workload\n")
        .expect("write invalid unified membership");

    let error = collect_pressure(&procfs, &sys, 101).expect_err("reject hierarchy ID");

    assert!(error.to_string().contains("no single valid unified"));
}

#[test]
fn pressure_omits_a_missing_resource_and_rejects_a_malformed_one() {
    let (dir, procfs, sys) = fixture_roots();
    let cgroup = prepare_v2_pressure(&dir, "/workload");
    std::fs::write(cgroup.join("cpu.pressure"), CPU_PRESSURE).expect("write CPU pressure");
    std::fs::write(cgroup.join("io.pressure"), IO_PRESSURE).expect("write I/O pressure");

    let rows = collect_pressure(&procfs, &sys, 100).expect("collect partial pressure");
    assert_eq!(
        rows.iter().map(|row| row.resource).collect::<Vec<_>>(),
        [0, 2]
    );

    std::fs::write(cgroup.join("io.pressure"), "some total=invalid\n")
        .expect("write malformed pressure");
    assert!(collect_pressure(&procfs, &sys, 101).is_err());
}

#[test]
fn pressure_omits_cgroup_v1_without_reading_host_pressure() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(dir.path().join("proc/self/cgroup"), "2:cpu:/workload\n")
        .expect("write v1 membership");
    std::fs::create_dir_all(dir.path().join("proc/pressure")).expect("mkdir host pressure");
    std::fs::write(dir.path().join("proc/pressure/cpu"), CPU_PRESSURE)
        .expect("write host pressure");

    let rows = collect_pressure(&procfs, &sys, 102).expect("collect unsupported cgroup pressure");

    assert!(rows.is_empty());
}

fn collect_v2_pids_fixture(current: Option<&str>, max: Option<&str>) -> CgroupCollection {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let workload = root.join("workload");
    std::fs::create_dir_all(&workload).expect("mkdir v2 pids fixture");
    std::fs::write(root.join("cgroup.controllers"), "pids\n").expect("write v2 controllers");
    write_optional_file(&workload.join("pids.current"), current);
    write_optional_file(&workload.join("pids.max"), max);

    collect(&SysFs::new(dir.path().to_path_buf()), 7, 100)
}

fn collect_v1_pids_fixture(current: Option<&str>, max: Option<&str>) -> CgroupCollection {
    let dir = tempfile::tempdir().expect("tempdir");
    let workload = dir.path().join("fs/cgroup/pids/workload");
    std::fs::create_dir_all(&workload).expect("mkdir v1 pids fixture");
    write_optional_file(&workload.join("pids.current"), current);
    write_optional_file(&workload.join("pids.max"), max);

    collect(&SysFs::new(dir.path().to_path_buf()), 7, 100)
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

    let rows = collect_workloads(&procfs, &sys, 7, 100).expect("collect workloads");

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

    let err = collect_workloads(&procfs, &sys, 7, 100).expect_err("candidate limit");

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

    let err = collect_workloads(&procfs, &sys, 7, 100).expect_err("path byte limit");

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

    let rows = collect_workloads(&procfs, &sys, 7, 100).expect("collect workloads");

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.pids.len(), 1);
    assert!(rows.io.is_empty());
    assert!(rows.io_omitted);
}

#[test]
fn workload_v1_keeps_controller_memberships_separate() {
    let (dir, procfs, sys) = fixture_roots();
    write_process_membership(
        &dir,
        101,
        "2:cpu,cpuacct:/service/cpu\n3:memory:/service/memory\n\
         4:blkio:/service/io\n5:pids:/service/tasks\n",
    );
    let cpu = fixture_cgroup_path(&dir, "cpu,cpuacct", "/service/cpu");
    let memory = fixture_cgroup_path(&dir, "memory", "/service/memory");
    let io = fixture_cgroup_path(&dir, "blkio", "/service/io");
    let pids = fixture_cgroup_path(&dir, "pids", "/service/tasks");
    for path in [&cpu, &memory, &io, &pids] {
        std::fs::create_dir_all(path).expect("mkdir v1 workload");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write cpu usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n").expect("write cpu stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n").expect("write memory usage");
    std::fs::write(pids.join("pids.current"), "9\n").expect("write pids current");
    std::fs::write(pids.join("pids.max"), "max\n").expect("write pids max");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write io bytes");
    std::fs::write(
        io.join("blkio.throttle.io_serviced"),
        "8:0 Read 3\n8:0 Write 4\n",
    )
    .expect("write io operations");

    let rows = collect_workloads(&procfs, &sys, 7, 100).expect("collect v1 workloads");

    assert_eq!(rows.cpu[0].cgroup_path, "/service/cpu");
    assert_eq!(rows.memory[0].cgroup_path, "/service/memory");
    assert_eq!(rows.io[0].cgroup_path, "/service/io");
    assert_eq!(rows.pids[0].cgroup_path, "/service/tasks");
    assert_eq!(rows.pids[0].max, None);
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
fn v1_pids_omits_rows_without_a_valid_current_value() {
    for current in [None, Some("invalid\n"), Some("-1\n")] {
        let rows = collect_v1_pids_fixture(current, Some("128\n"));
        assert!(rows.pids.is_empty(), "current={current:?}");
    }
}

#[test]
fn v1_pids_omits_rows_without_a_valid_max_value() {
    for max in [None, Some("invalid\n"), Some("-1\n")] {
        let rows = collect_v1_pids_fixture(Some("9\n"), max);
        assert!(rows.pids.is_empty(), "max={max:?}");
    }
}

#[test]
fn context_v2_uses_the_unified_self_path_and_effective_cpuset() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/kubepods/pod-a/container-a\n",
    )
    .expect("write self cgroup");
    let cgroup = dir.path().join("sys/fs/cgroup/kubepods/pod-a/container-a");
    std::fs::create_dir_all(&cgroup).expect("mkdir unified cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io cpuset\n",
    )
    .expect("write controllers");
    std::fs::write(cgroup.join("cpuset.cpus.effective"), "0-2,5,8-9\n")
        .expect("write effective cpuset");
    std::fs::write(
        cgroup.join("cpu.stat"),
        "usage_usec 10\nuser_usec 6\nsystem_usec 4\n",
    )
    .expect("write cpu stat");
    std::fs::write(cgroup.join("memory.current"), "4096\n").expect("write memory current");
    std::fs::write(
        cgroup.join("memory.stat"),
        "anon 100\nfile 200\nkernel 50\nslab 20\n",
    )
    .expect("write memory stat");
    std::fs::write(
        cgroup.join("io.stat"),
        "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n",
    )
    .expect("write io stat");

    let context = collect_context(&procfs, &sys, 99).expect("collect context");

    assert_eq!(context.ts, 99);
    assert_eq!(context.cgroup_version, 2);
    assert_eq!(
        context.cpu_path.as_deref(),
        Some("/kubepods/pod-a/container-a")
    );
    assert_eq!(context.memory_path, context.cpu_path);
    assert_eq!(context.io_path, context.cpu_path);
    assert_eq!(context.cpuset_cpus, Some(6));
}

#[test]
fn context_v1_keeps_controller_specific_paths() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu,cpuacct:/service/cpu\n3:memory:/service/memory\n\
         4:blkio:/service/io\n5:cpuset:/service/set\n",
    )
    .expect("write self cgroup");
    let cpuset = dir.path().join("sys/fs/cgroup/cpuset/service/set");
    std::fs::create_dir_all(&cpuset).expect("mkdir cpuset cgroup");
    std::fs::write(cpuset.join("cpuset.effective_cpus"), "1,3-4\n")
        .expect("write effective cpuset");
    let cpu = dir.path().join("sys/fs/cgroup/cpu,cpuacct/service/cpu");
    let memory = dir.path().join("sys/fs/cgroup/memory/service/memory");
    let io = dir.path().join("sys/fs/cgroup/blkio/service/io");
    for path in [&cpu, &memory, &io] {
        std::fs::create_dir_all(path).expect("mkdir controller cgroup");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write cpu usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n").expect("write cpu account stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n").expect("write memory current");
    std::fs::write(memory.join("memory.limit_in_bytes"), "16384\n").expect("write memory limit");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 100\ntotal_cache 200\ntotal_slab 20\ntotal_kernel_stack 5\n\
         hierarchical_memory_limit 8192\n",
    )
    .expect("write memory stat");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write io bytes");
    std::fs::write(
        io.join("blkio.throttle.io_serviced"),
        "8:0 Read 3\n8:0 Write 4\n",
    )
    .expect("write io operations");

    let context = collect_context(&procfs, &sys, 123).expect("collect context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path.as_deref(), Some("/service/cpu"));
    assert_eq!(context.memory_path.as_deref(), Some("/service/memory"));
    assert_eq!(context.io_path.as_deref(), Some("/service/io"));
    assert_eq!(context.cpuset_cpus, Some(3));
}

#[test]
fn hybrid_membership_uses_the_v1_tree_selected_by_collection() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/unified/workload\n2:cpu,cpuacct:/service/cpu\n\
         3:memory:/service/memory\n4:blkio:/service/io\n5:cpuset:/service/set\n",
    )
    .expect("write hybrid self cgroup");
    let unified = dir.path().join("sys/fs/cgroup/unified");
    std::fs::create_dir_all(&unified).expect("mkdir nested unified mount");
    std::fs::write(unified.join("cgroup.controllers"), "cpu memory io cpuset\n")
        .expect("write nested unified controllers");
    std::fs::write(unified.join("cpu.max"), "1 100000\n").expect("write unrelated unified quota");

    let cpu = dir.path().join("sys/fs/cgroup/cpu,cpuacct/service/cpu");
    let memory = dir.path().join("sys/fs/cgroup/memory/service/memory");
    let io = dir.path().join("sys/fs/cgroup/blkio/service/io");
    let cpuset = dir.path().join("sys/fs/cgroup/cpuset/service/set");
    for path in [&cpu, &memory, &io, &cpuset] {
        std::fs::create_dir_all(path).expect("mkdir hybrid controller cgroup");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write cpu usage");
    write_v1_cpu_capacity(&dir, "/", "-1\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service", "200000\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service/cpu", "300000\n", "100000\n");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n").expect("write cpu account stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n").expect("write memory current");
    std::fs::write(memory.join("memory.limit_in_bytes"), "16384\n").expect("write memory limit");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 100\ntotal_cache 200\ntotal_slab 20\ntotal_kernel_stack 5\n\
         hierarchical_memory_limit 8192\n",
    )
    .expect("write memory stat");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write io bytes");
    std::fs::write(
        io.join("blkio.throttle.io_serviced"),
        "8:0 Read 3\n8:0 Write 4\n",
    )
    .expect("write io operations");
    std::fs::write(cpuset.join("cpuset.effective_cpus"), "0-1\n").expect("write effective cpuset");

    let context = collect_context(&procfs, &sys, 123).expect("collect hybrid context");
    let rows = collect(&sys, 123, 100);

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path.as_deref(), Some("/service/cpu"));
    assert_eq!(context.memory_path.as_deref(), Some("/service/memory"));
    assert_eq!(context.io_path.as_deref(), Some("/service/io"));
    assert_eq!(context.cpuset_cpus, Some(2));
    assert_eq!(context.effective_cpu_quota_usec, Some(200_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(8192));
    assert!(rows.cpu.iter().any(|row| row.cgroup_path == "/service/cpu"));
    assert!(
        rows.memory
            .iter()
            .any(|row| row.cgroup_path == "/service/memory")
    );
    assert!(rows.io.iter().any(|row| row.cgroup_path == "/service/io"));
}

#[test]
fn context_does_not_combine_different_v1_cpu_controller_paths() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu:/service/quota\n3:cpuacct:/service/usage\n",
    )
    .expect("write self cgroup");

    let context = collect_context(&procfs, &sys, 1).expect("collect context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.cpuset_cpus, None);
}

#[test]
fn context_keeps_unavailable_or_malformed_cpuset_null() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(dir.path().join("proc/self/cgroup"), "0::/workload\n")
        .expect("write self cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu cpuset\n",
    )
    .expect("write controllers");
    let workload = dir.path().join("sys/fs/cgroup/workload");
    std::fs::create_dir_all(&workload).expect("mkdir workload");
    std::fs::write(workload.join("cpuset.cpus.effective"), "0-2,2\n")
        .expect("write malformed effective cpuset");

    let context = collect_context(&procfs, &sys, 1).expect("collect context");

    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpuset_cpus, None);
}

#[test]
fn missing_self_membership_is_reported_even_on_a_v2_mount() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write v2 controllers");

    assert_eq!(
        collect_context(&procfs, &sys, 7)
            .expect_err("missing membership must be reported")
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn partial_v1_io_counters_keep_the_exact_controller_path() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu:/partial\n3:memory:/partial\n4:blkio:/partial\n",
    )
    .expect("write self cgroup");
    let cpu = dir.path().join("sys/fs/cgroup/cpu/partial");
    let memory = dir.path().join("sys/fs/cgroup/memory/partial");
    let io = dir.path().join("sys/fs/cgroup/blkio/partial");
    for path in [&cpu, &memory, &io] {
        std::fs::create_dir_all(path).expect("mkdir partial controller cgroup");
    }
    std::fs::write(cpu.join("cpu.stat"), "nr_throttled 1\n").expect("write partial cpu stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n")
        .expect("write partial memory current");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write partial io bytes");

    let context = collect_context(&procfs, &sys, 7).expect("collect partial context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.memory_path, None);
    assert_eq!(context.io_path.as_deref(), Some("/partial"));
}

#[test]
fn partial_v2_io_counters_keep_the_unified_path() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(dir.path().join("proc/self/cgroup"), "0::/partial\n")
        .expect("write self cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write controllers");
    let cgroup = dir.path().join("sys/fs/cgroup/partial");
    std::fs::create_dir_all(&cgroup).expect("mkdir partial cgroup");
    std::fs::write(cgroup.join("cpu.stat"), "usage_usec 10\n").expect("write partial cpu stat");
    std::fs::write(cgroup.join("memory.current"), "4096\n").expect("write partial memory current");
    std::fs::write(cgroup.join("io.stat"), "8:0 rbytes=1 wbytes=2\n")
        .expect("write partial io stat");

    let context = collect_context(&procfs, &sys, 7).expect("collect partial context");

    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.memory_path, None);
    assert_eq!(context.io_path.as_deref(), Some("/partial"));
}

#[test]
fn context_v2_uses_parent_stricter_cpu_and_memory_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    write_v2_capacity(&dir, "/team", "100000 100000\n", "4096\n");
    write_v2_capacity(&dir, "/team/workload", "300000 100000\n", "8192\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v2 parent limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(100_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(4096));
}

#[test]
fn context_v2_includes_present_mount_root_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    write_v2_capacity(&dir, "/", "50000 100000\n", "1024\n");
    write_v2_capacity(&dir, "/team", "100000 100000\n", "4096\n");
    write_v2_capacity(&dir, "/team/workload", "200000 100000\n", "8192\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v2 mount-root limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(50_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(1024));
}

#[test]
fn context_v2_root_membership_without_limit_files_is_unknown() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/");

    let context = collect_context(&procfs, &sys, 10).expect("collect v2 root membership");

    assert_eq!(context.cpu_path.as_deref(), Some("/"));
    assert_eq!(context.memory_path.as_deref(), Some("/"));
    assert_eq!(context.effective_cpu_quota_usec, None);
    assert_eq!(context.effective_cpu_period_usec, None);
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v2_does_not_ignore_malformed_mount_root_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    write_v2_capacity(&dir, "/", "max invalid\n", "invalid\n");
    write_v2_capacity(&dir, "/team", "100000 100000\n", "4096\n");
    write_v2_capacity(&dir, "/team/workload", "200000 100000\n", "8192\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect malformed v2 mount root");

    assert_eq!(context.effective_cpu_quota_usec, None);
    assert_eq!(context.effective_cpu_period_usec, None);
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v2_compares_cpu_ratios_and_uses_leaf_stricter_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    write_v2_capacity(&dir, "/team", "50000 10000\n", "8192\n");
    write_v2_capacity(&dir, "/team/workload", "100000 100000\n", "4096\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v2 leaf limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(100_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(4096));
}

#[test]
fn context_v2_distinguishes_validated_unlimited_cpu_from_unknown() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    write_v2_capacity(&dir, "/team", "max 50000\n", "max\n");
    write_v2_capacity(&dir, "/team/workload", "max 200000\n", "max\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v2 unlimited limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(-1));
    assert_eq!(context.effective_cpu_period_usec, Some(200_000));
    assert_eq!(context.effective_memory_max, None);
    assert_eq!(context.cpuset_cpus, Some(2));
}

#[test]
fn context_v2_keeps_incoherent_hierarchies_unknown() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/team/workload");
    let parent = fixture_cgroup_path(&dir, "", "/team");
    std::fs::create_dir_all(&parent).expect("mkdir malformed v2 parent");
    std::fs::write(parent.join("cpu.max"), "50000 invalid\n").expect("write malformed v2 CPU max");
    write_v2_capacity(&dir, "/team/workload", "100000 100000\n", "4096\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect incoherent v2 hierarchy");

    assert!(context.cpu_path.is_some());
    assert!(context.memory_path.is_some());
    assert_eq!(context.effective_cpu_quota_usec, None);
    assert_eq!(context.effective_cpu_period_usec, None);
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v1_uses_kernel_parent_stricter_memory_and_cpu_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "8192\n", "4096");
    write_v1_cpu_capacity(&dir, "/", "-1\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service", "100000\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service/workload", "300000\n", "100000\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v1 parent limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(100_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(4096));
}

#[test]
fn context_v1_compares_cpu_ratios_and_uses_leaf_stricter_limits() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "2048\n", "2048");
    write_v1_cpu_capacity(&dir, "/", "-1\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service", "50000\n", "10000\n");
    write_v1_cpu_capacity(&dir, "/service/workload", "100000\n", "100000\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v1 leaf limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(100_000));
    assert_eq!(context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context.effective_memory_max, Some(2048));
}

#[test]
fn context_v1_handles_unlimited_sentinels() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(
        &dir,
        "/service/workload",
        "9223372036854771712\n",
        "9223372036854771712",
    );
    write_v1_cpu_capacity(&dir, "/", "-1\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service", "-1\n", "50000\n");
    write_v1_cpu_capacity(&dir, "/service/workload", "-1\n", "200000\n");

    let context = collect_context(&procfs, &sys, 10).expect("collect v1 unlimited limits");

    assert_eq!(context.effective_cpu_quota_usec, Some(-1));
    assert_eq!(context.effective_cpu_period_usec, Some(200_000));
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn v1_capacity_limit_accepts_one_whitespace_terminated_value_only() {
    assert_eq!(
        parse_v1_capacity_limit("4096\n"),
        Some(MemoryLimit::Limited(4096))
    );
    assert_eq!(
        parse_v1_capacity_limit("-1\n"),
        Some(MemoryLimit::Unlimited)
    );
    assert_eq!(parse_v1_capacity_limit("4096 extra\n"), None);
}

#[test]
fn context_v1_rejects_ambiguous_roots_and_malformed_hierarchy_values() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "8192\n", "not-a-limit");
    write_v1_cpu_capacity(&dir, "/", "-1\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service", "100000\n", "100000\n");
    write_v1_cpu_capacity(&dir, "/service/workload", "100000\n", "100000\n");
    let duplicate = fixture_cgroup_path(&dir, "cpu", "/service/workload");
    std::fs::create_dir_all(&duplicate).expect("mkdir duplicate v1 CPU leaf");
    std::fs::write(duplicate.join("cpu.cfs_quota_us"), "50000\n")
        .expect("write duplicate v1 CPU quota");

    let context = collect_context(&procfs, &sys, 10).expect("collect ambiguous v1 hierarchy");

    assert!(context.cpu_path.is_some());
    assert!(context.memory_path.is_some());
    assert_eq!(context.effective_cpu_quota_usec, None);
    assert_eq!(context.effective_cpu_period_usec, None);
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v1_rejects_hierarchical_memory_above_finite_leaf_limit() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "2048\n", "4096");

    let context = collect_context(&procfs, &sys, 10).expect("collect mismatched v1 memory");

    assert!(context.memory_path.is_some());
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v1_rejects_unlimited_hierarchy_with_finite_leaf_limit() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "2048\n", "9223372036854771712");

    let context = collect_context(&procfs, &sys, 10).expect("collect invalid v1 unlimited value");

    assert!(context.memory_path.is_some());
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn context_v1_does_not_mix_memory_files_between_controller_roots() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v1_context(&dir, "/service/workload", "8192\n", "4096");
    let memory = fixture_cgroup_path(&dir, "memory", "/service/workload");
    std::fs::remove_file(memory.join("memory.limit_in_bytes"))
        .expect("remove bound-root memory limit");
    let unrelated = fixture_cgroup_path(&dir, "", "/service/workload");
    std::fs::create_dir_all(&unrelated).expect("mkdir unrelated memory leaf");
    std::fs::write(unrelated.join("memory.limit_in_bytes"), "8192\n")
        .expect("write unrelated memory limit");

    let context = collect_context(&procfs, &sys, 10).expect("collect mixed-root v1 memory");

    assert!(context.memory_path.is_some());
    assert_eq!(context.effective_memory_max, None);
}

#[test]
fn collect_v2_reads_every_controller_file() {
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
    let rows = collect(&sys, 99, 100);

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
fn collect_v1_reads_every_controller_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let cpu = root.join("cpu,cpuacct/workload");
    let memory = root.join("memory/workload");
    let pids = root.join("pids/workload");
    let blkio = root.join("blkio/workload");
    for path in [&cpu, &memory, &pids, &blkio] {
        std::fs::create_dir_all(path).expect("mkdir cgroup controller");
    }

    std::fs::write(cpu.join("cpuacct.usage"), "200000000\n").expect("write cpuacct.usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 30\nsystem 20\n").expect("write cpuacct.stat");
    std::fs::write(cpu.join("cpu.cfs_quota_us"), "50000\n").expect("write cpu.cfs_quota_us");
    std::fs::write(cpu.join("cpu.cfs_period_us"), "100000\n").expect("write cpu.cfs_period_us");
    std::fs::write(
        cpu.join("cpu.stat"),
        "nr_periods 9\nnr_throttled 3\nthrottled_time 700000000\n",
    )
    .expect("write cpu.stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "8192\n")
        .expect("write memory.usage_in_bytes");
    std::fs::write(memory.join("memory.limit_in_bytes"), "16384\n")
        .expect("write memory.limit_in_bytes");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 1000\ntotal_cache 2000\ntotal_slab 300\ntotal_kernel_stack 40\n",
    )
    .expect("write memory.stat");
    std::fs::write(memory.join("memory.failcnt"), "6\n").expect("write memory.failcnt");
    std::fs::write(pids.join("pids.current"), "9\n").expect("write pids.current");
    std::fs::write(pids.join("pids.max"), "128\n").expect("write pids.max");
    std::fs::write(
        blkio.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 10\n8:0 Write 20\n259:0 Read 30\n259:0 Write 40\n",
    )
    .expect("write blkio bytes");
    std::fs::write(
        blkio.join("blkio.throttle.io_serviced"),
        "8:0 Read 1\n8:0 Write 2\n259:0 Read 3\n259:0 Write 4\n",
    )
    .expect("write blkio ops");

    let sys = SysFs::new(dir.path().to_path_buf());
    let rows = collect(&sys, 123, 100);

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.pids.len(), 1);
    assert_eq!(rows.io.len(), 2);

    let cpu = &rows.cpu[0];
    assert_eq!(cpu.cgroup_path, "/workload");
    assert_eq!(cpu.ts, 123);
    assert_eq!(cpu.usage_usec, 200_000);
    assert_eq!(cpu.user_usec, 300_000);
    assert_eq!(cpu.system_usec, 200_000);
    assert_eq!(cpu.nr_throttled, 3);
    assert_eq!(cpu.throttled_usec, 700_000);
    assert_eq!(cpu.quota_usec, 50_000);
    assert_eq!(cpu.period_usec, 100_000);

    let memory = &rows.memory[0];
    assert_eq!(memory.current, 8192);
    assert_eq!(memory.max, Some(16_384));
    assert_eq!(memory.anon, 1000);
    assert_eq!(memory.file, 2000);
    assert_eq!(memory.slab, 300);
    assert_eq!(memory.kernel, 340);
    assert_eq!(memory.max_events, 6);

    assert_eq!(rows.pids[0].current, 9);
    assert_eq!(rows.pids[0].max, Some(128));
    assert_eq!((rows.io[0].major, rows.io[0].minor), (8, 0));
    assert_eq!(rows.io[0].rbytes, Some(10));
    assert_eq!(rows.io[0].wbytes, Some(20));
    assert_eq!(rows.io[0].rios, Some(1));
    assert_eq!(rows.io[0].wios, Some(2));
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
    let context = CgroupContextRow {
        ts: 7,
        cgroup_version: 2,
        cpu_path: Some("/workload".to_owned()),
        memory_path: Some("/workload".to_owned()),
        io_path: Some("/workload".to_owned()),
        cpuset_cpus: Some(4),
        effective_cpu_quota_usec: Some(150_000),
        effective_cpu_period_usec: Some(100_000),
        effective_memory_max: Some(536_870_912),
    };

    let context_section = to_context_section(
        &context,
        3,
        Some(cgroup_path),
        Some(cgroup_path),
        Some(cgroup_path),
    );
    assert_eq!(context_section.ts, Ts(7));
    assert_eq!(context_section.cgroup_version, 2);
    assert_eq!(context_section.cpu_path, Some(cgroup_path));
    assert_eq!(context_section.cpuset_cpus, Some(4));
    assert_eq!(context_section.effective_cpu_quota_usec, Some(150_000));
    assert_eq!(context_section.effective_cpu_period_usec, Some(100_000));
    assert_eq!(context_section.effective_memory_max, Some(536_870_912));
    assert_eq!(context_section.scope, 3);

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
fn charged_devices_lists_the_io_stat_devices_of_the_own_v2_cgroup() {
    let (dir, procfs, sys) = fixture_roots();
    prepare_v2_context(&dir, "/workload");
    std::fs::write(
        fixture_cgroup_path(&dir, "", "/workload").join("io.stat"),
        "252:0 rbytes=1 wbytes=2 rios=3 wios=4\n259:0 rbytes=1 wbytes=2 rios=3 wios=4\n",
    )
    .expect("write layered io stat");

    assert_eq!(
        charged_devices(&procfs, &sys).expect("read charged devices"),
        [(252, 0), (259, 0)]
    );
}

#[test]
fn charged_devices_reports_missing_v2_io_stat_but_omits_v1() {
    let (dir, procfs, sys) = fixture_roots();
    assert!(
        charged_devices(&procfs, &sys)
            .expect("no v2 hierarchy")
            .is_empty()
    );

    prepare_v2_context(&dir, "/workload");
    std::fs::remove_file(fixture_cgroup_path(&dir, "", "/workload").join("io.stat"))
        .expect("remove io stat");
    let error = charged_devices(&procfs, &sys).expect_err("missing io.stat");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains("workload/io.stat"));
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
        assert!(collect_pressure(&procfs, &sys, 1).is_err(), "{membership}");
        assert_eq!(
            charged_devices(&procfs, &sys)
                .expect_err("invalid membership")
                .kind(),
            io::ErrorKind::InvalidData,
            "{membership}",
        );
        let context = collect_context(&procfs, &sys, 1).expect("collect context");
        assert_eq!(context.cpu_path, None, "{membership}");
        assert_eq!(context.memory_path, None, "{membership}");
        assert_eq!(context.io_path, None, "{membership}");
        let mut memberships = WorkloadMemberships::new(&sys);
        memberships.observe(membership);
        let workload = memberships.collect(&sys, 1, 100).expect("collect workload");
        assert!(workload.cpu.is_empty(), "{membership}");
        assert!(workload.memory.is_empty(), "{membership}");
        assert!(workload.io.is_empty(), "{membership}");
        assert!(workload.pids.is_empty(), "{membership}");
    }
}
