use super::*;
use std::fmt::Write as _;
use std::path::Path;

fn fixture() -> (tempfile::TempDir, ProcFs, SysFs) {
    let dir = tempfile::tempdir().expect("temporary tree");
    std::fs::create_dir_all(dir.path().join("proc/self")).expect("proc tree");
    std::fs::create_dir_all(dir.path().join("sys/fs/cgroup")).expect("cgroup tree");
    let procfs = ProcFs::new(dir.path().join("proc"));
    let sys = SysFs::new(dir.path().join("sys"));
    (dir, procfs, sys)
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("fixture file");
}

fn v2(root: &Path, membership: &str, mount_root: &str) {
    write(root, "proc/self/cgroup", &format!("0::{membership}\n"));
    write(
        root,
        "sys/fs/cgroup/cgroup.controllers",
        "cpu memory io pids cpuset\n",
    );
    write(
        root,
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 {mount_root} {} rw - cgroup2 cgroup rw\n",
            root.join("sys/fs/cgroup").display()
        ),
    );
}

#[test]
fn highest_directory_wins_even_without_metrics_and_with_inaccessible_intermediate() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/blocked/collector", "/");
    // A non-directory prevents descendant traversal; the known root is still accessible.
    write(dir.path(), "sys/fs/cgroup/blocked", "not a directory");
    let selected = collect_ancestor_context(&procfs, &sys, 10).expect("select root");
    assert_eq!(selected.context.cpu_path.as_deref(), Some("/"));
    assert!(
        selected
            .cpu
            .as_ref()
            .expect("CPU identity")
            .identity
            .contains("fs/cgroup")
    );
    assert_eq!(selected.context.effective_cpu_quota_usec, None);
    let rows = collect_ancestor_rows(&sys, &selected, 10, 100);
    assert!(rows.ancestor_cpu.is_empty());
    assert!(rows.ancestor_memory.is_empty());
}

#[test]
fn primary_root_never_borrows_complete_child_metrics_or_pressure() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/collector", "/");
    write(
        dir.path(),
        "sys/fs/cgroup/pod/collector/cpu.stat",
        "usage_usec 999\nuser_usec 600\nsystem_usec 399\nnr_throttled 9\nthrottled_usec 99\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/pod/collector/cpu.max",
        "25000 100000",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/pod/collector/cpu.pressure",
        "some avg10=90 avg60=80 avg300=70 total=9000",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/cpu.stat",
        "usage_usec 100\nuser_usec 60\nsystem_usec 40\n",
    );
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("selection");
    let rows = collect_ancestor_rows(&sys, &selected, 1, 100);
    let cpu = &rows.ancestor_cpu[0];
    assert_eq!(cpu.cgroup_path, "/");
    assert_eq!(cpu.usage_usec, 100);
    assert_eq!(cpu.quota_usec, None);
    assert_eq!(cpu.throttled_usec, None);
    assert!(
        collect_ancestor_pressure(&sys, &selected, 1)
            .expect("missing root PSI")
            .is_empty()
    );
}

#[test]
fn selected_capacity_changes_in_time_without_reusing_the_child_quota() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/collector", "/");
    write(
        dir.path(),
        "sys/fs/cgroup/collector/cpu.max",
        "25000 100000",
    );
    write(dir.path(), "sys/fs/cgroup/cpu.max", "150000 100000");
    write(
        dir.path(),
        "sys/fs/cgroup/cpu.stat",
        "usage_usec 100\nuser_usec 60\nsystem_usec 40\n",
    );
    write(dir.path(), "sys/fs/cgroup/cpuset.cpus.effective", "0-7");
    let first = collect_ancestor_context(&procfs, &sys, 1).expect("first");
    assert_eq!(first.context.effective_cpu_quota_usec, Some(150_000));
    assert_eq!(first.context.effective_cpu_period_usec, Some(100_000));
    assert_eq!(first.context.cpuset_cpus, Some(8));
    assert_eq!(
        collect_ancestor_rows(&sys, &first, 1, 100).ancestor_cpu[0].quota_usec,
        Some(150_000)
    );
    write(dir.path(), "sys/fs/cgroup/cpu.max", "200000 100000");
    let second = collect_ancestor_context(&procfs, &sys, 2).expect("second");
    assert_eq!(second.context.effective_cpu_quota_usec, Some(200_000));
    assert_eq!(
        collect_ancestor_rows(&sys, &second, 2, 100).ancestor_cpu[0].quota_usec,
        Some(200_000)
    );
    assert_eq!(
        first.cpu.as_ref().expect("first CPU").identity,
        second.cpu.as_ref().expect("second CPU").identity
    );
}

#[test]
fn hidden_mount_ancestors_do_not_erase_observed_parent_limits() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/collector", "/pod");
    write(
        dir.path(),
        "sys/fs/cgroup/collector/cpu.max",
        "25000 100000",
    );
    write(dir.path(), "sys/fs/cgroup/cpu.max", "400000 100000");
    write(dir.path(), "sys/fs/cgroup/memory.max", "1000000");
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("select mounted root");
    assert_eq!(selected.cpu.as_ref().expect("CPU group").root, "/pod");
    assert_eq!(selected.context.cpu_path.as_deref(), Some("/"));
    assert_eq!(selected.context.effective_cpu_quota_usec, Some(400_000));
    assert_eq!(selected.context.effective_memory_max, Some(1_000_000));
}

#[test]
fn selected_v1_memory_preserves_hierarchical_values_and_unknown_events() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "proc/self/cgroup",
        "2:memory:/pod/collector\n3:cpu:/pod/collector\n4:cpuacct:/pod/collector\n5:pids:/other/collector\n",
    );
    for controller in ["memory", "cpu", "cpuacct", "pids"] {
        std::fs::create_dir_all(dir.path().join(format!("sys/fs/cgroup/{controller}")))
            .expect("controller root");
    }
    write(
        dir.path(),
        "sys/fs/cgroup/memory/memory.usage_in_bytes",
        "4096",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/memory/memory.stat",
        "total_rss 100\nrss 1\ntotal_cache 200\ncache 2\ntotal_slab 30\nslab 3\ntotal_kernel_stack 4\nhierarchical_memory_limit 8192\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/memory/memory.limit_in_bytes",
        "16384",
    );
    write(dir.path(), "sys/fs/cgroup/pids/pids.current", "8");
    write(dir.path(), "sys/fs/cgroup/pids/pids.max", "max");
    let mut mounts = String::new();
    for (index, controller) in ["memory", "cpu", "cpuacct", "pids"].into_iter().enumerate() {
        writeln!(
            &mut mounts,
            "{} 1 0:{} / {} rw - cgroup cgroup rw,{controller}",
            50 + index,
            40 + index,
            dir.path()
                .join(format!("sys/fs/cgroup/{controller}"))
                .display()
        )
        .expect("format mountinfo fixture");
    }
    write(dir.path(), "proc/self/mountinfo", &mounts);
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("v1 selection");
    assert!(
        !selected
            .cpu
            .as_ref()
            .expect("accounting scope")
            .cpu_bandwidth,
        "separate bandwidth tree cannot constrain accounting automatically"
    );
    assert_eq!(selected.context.effective_cpu_quota_usec, None);
    assert_ne!(
        selected.memory.as_ref().expect("memory").identity,
        selected.pids.as_ref().expect("pids").identity
    );
    assert_eq!(selected.context.effective_memory_max, Some(8192));
    let rows = collect_ancestor_rows(&sys, &selected, 1, 100);
    let memory = &rows.ancestor_memory[0];
    assert_eq!(memory.current, 4096);
    assert_eq!(
        (memory.anon, memory.file, memory.kernel, memory.slab),
        (Some(100), Some(200), Some(34), Some(30))
    );
    assert_eq!(memory.low_events, None);
    assert_eq!(memory.oom_kill, None);
    assert_eq!(rows.pids[0].current, 8);
}

#[test]
fn missing_and_invalid_memory_fields_remain_null_without_losing_current() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/collector", "/");
    write(dir.path(), "sys/fs/cgroup/memory.current", "42");
    write(dir.path(), "sys/fs/cgroup/memory.max", "invalid");
    write(
        dir.path(),
        "sys/fs/cgroup/memory.stat",
        "anon 0\nfile invalid\nslab -1\n",
    );
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("selected");
    let rows = collect_ancestor_rows(&sys, &selected, 1, 100);
    let memory = &rows.ancestor_memory[0];
    assert_eq!(memory.current, 42);
    assert_eq!(memory.anon, Some(0));
    assert_eq!(memory.file, None);
    assert_eq!(memory.slab, None);
    assert_eq!(memory.max, None);
    assert_eq!(memory.max_unlimited, None);
}

#[test]
fn selection_identity_changes_when_the_directory_is_replaced() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/collector", "/");
    let first = collect_ancestor_context(&procfs, &sys, 1).expect("first");
    std::fs::rename(
        dir.path().join("sys/fs/cgroup"),
        dir.path().join("sys/fs/old-cgroup"),
    )
    .expect("retain old directory");
    std::fs::create_dir(dir.path().join("sys/fs/cgroup")).expect("replacement");
    write(
        dir.path(),
        "sys/fs/cgroup/cpu.stat",
        "usage_usec 900\nuser_usec 600\nsystem_usec 300\n",
    );
    assert!(
        collect_ancestor_rows(&sys, &first, 2, 100)
            .ancestor_cpu
            .is_empty()
    );
    let second = collect_ancestor_context(&procfs, &sys, 2).expect("second");
    assert_eq!(first.context.cpu_path, second.context.cpu_path);
    assert_ne!(
        first.cpu.expect("first CPU").identity,
        second.cpu.expect("second CPU").identity
    );
}

#[test]
fn an_unrelated_mounted_subtree_is_not_an_ancestor() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/workload/collector", "/other");
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("read membership and mount");
    assert!(selected.cpu.is_none());
    assert!(selected.memory.is_none());
}

#[test]
fn missing_mountinfo_cannot_invent_a_controller_binding() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "proc/self/cgroup", "0::/collector");
    write(dir.path(), "sys/fs/cgroup/cgroup.controllers", "cpu memory");
    assert!(collect_ancestor_context(&procfs, &sys, 1).is_err());
}

#[test]
fn child_churn_does_not_change_parent_counter_identity() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/collector", "/");
    let first = collect_ancestor_context(&procfs, &sys, 1).expect("first selection");
    std::fs::create_dir(dir.path().join("sys/fs/cgroup/new-child")).expect("new child");
    let second = collect_ancestor_context(&procfs, &sys, 2).expect("second selection");
    assert_eq!(
        first.cpu.expect("first parent").identity,
        second.cpu.expect("second parent").identity
    );
}

#[test]
fn missing_quota_keeps_the_same_scope_cpuset_and_unlimited_memory_has_no_finite_bound() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/collector", "/");
    write(dir.path(), "sys/fs/cgroup/cpuset.cpus.effective", "0-1");
    write(dir.path(), "sys/fs/cgroup/memory.max", "max");
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("selection");
    assert_eq!(selected.context.effective_cpu_quota_usec, None);
    assert_eq!(selected.context.cpuset_cpus, Some(2));
    assert_eq!(selected.context.effective_memory_max, None);
    write(dir.path(), "sys/fs/cgroup/cpu.max", "invalid 100000");
    write(dir.path(), "sys/fs/cgroup/memory.max", "invalid");
    let invalid = collect_ancestor_context(&procfs, &sys, 2).expect("invalid fields");
    assert_eq!(invalid.context.effective_cpu_quota_usec, None);
    assert_eq!(invalid.context.cpuset_cpus, Some(2));
    assert_eq!(invalid.context.effective_memory_max, None);
}

#[test]
fn v1_coherent_cpuset_is_independent_of_an_unrelated_bandwidth_tree() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "proc/self/cgroup",
        "2:cpuacct,cpuset:/collector\n3:cpu:/elsewhere\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/accounting/cpuset.effective_cpus",
        "0-1",
    );
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup cgroup rw,cpuacct,cpuset\n",
            dir.path().join("sys/fs/cgroup/accounting").display()
        ),
    );
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("bound accounting and cpuset");
    assert_eq!(selected.context.cpu_path.as_deref(), Some("/"));
    assert_eq!(selected.context.effective_cpu_quota_usec, None);
    assert_eq!(selected.context.cpuset_cpus, Some(2));
}
