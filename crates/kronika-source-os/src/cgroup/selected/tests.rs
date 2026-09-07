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
    let rows = collect_ancestor_rows(&sys, &selected, 10);
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
    let rows = collect_ancestor_rows(&sys, &selected, 1);
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
        collect_ancestor_rows(&sys, &first, 1).ancestor_cpu[0].quota_usec,
        Some(150_000)
    );
    write(dir.path(), "sys/fs/cgroup/cpu.max", "200000 100000");
    let second = collect_ancestor_context(&procfs, &sys, 2).expect("second");
    assert_eq!(second.context.effective_cpu_quota_usec, Some(200_000));
    assert_eq!(
        collect_ancestor_rows(&sys, &second, 2).ancestor_cpu[0].quota_usec,
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
    let rows = collect_ancestor_rows(&sys, &selected, 1);
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
        collect_ancestor_rows(&sys, &first, 2)
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
fn v2_unrelated_extra_mount_does_not_hide_the_compatible_root() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/collector", "/");
    std::fs::create_dir(dir.path().join("sys/fs/cgroup/extra")).expect("extra mount");
    let path = dir.path().join("proc/self/mountinfo");
    let mut text = std::fs::read_to_string(&path).expect("mountinfo");
    writeln!(
        &mut text,
        "41 1 0:30 /other {} rw - cgroup2 cgroup rw",
        dir.path().join("sys/fs/cgroup/extra").display()
    )
    .expect("extra binding");
    write(dir.path(), "proc/self/mountinfo", &text);
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("root remains selected");
    assert_eq!(selected.cpu.as_ref().expect("CPU root").base, "fs/cgroup");
}

#[test]
fn duplicate_and_subtree_bind_mounts_preserve_the_highest_actual_object() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/collector", "/");
    std::fs::create_dir_all(dir.path().join("sys/fs/cgroup/pod/collector")).expect("pod tree");
    let mut text =
        std::fs::read_to_string(dir.path().join("proc/self/mountinfo")).expect("mountinfo");
    writeln!(
        &mut text,
        "41 1 0:30 / {} rw - cgroup2 cgroup rw",
        dir.path().join("sys/fs/cgroup").display()
    )
    .expect("duplicate binding");
    writeln!(
        &mut text,
        "42 1 0:30 /pod {} rw - cgroup2 cgroup rw",
        dir.path().join("sys/fs/cgroup/pod").display()
    )
    .expect("subtree binding");
    write(dir.path(), "proc/self/mountinfo", &text);
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("highest selection");
    assert_eq!(selected.cpu.as_ref().expect("CPU root").base, "fs/cgroup");
    assert_eq!(selected.context.cpu_path.as_deref(), Some("/"));
}

#[test]
fn compatible_mount_paths_with_different_objects_remain_ambiguous() {
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/collector", "/");
    std::fs::create_dir(dir.path().join("sys/fs/cgroup/extra")).expect("other object");
    let mut text =
        std::fs::read_to_string(dir.path().join("proc/self/mountinfo")).expect("mountinfo");
    writeln!(
        &mut text,
        "41 1 0:31 / {} rw - cgroup2 cgroup rw",
        dir.path().join("sys/fs/cgroup/extra").display()
    )
    .expect("different hierarchy binding");
    write(dir.path(), "proc/self/mountinfo", &text);
    let selected = collect_ancestor_context(&procfs, &sys, 1).expect("ambiguous selection");
    assert!(selected.cpu.is_none());
}

#[test]
fn v1_files_cannot_supply_selected_rows_capacity_pressure_or_devices() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "proc/self/cgroup",
        "2:cpu,cpuacct,memory,blkio,pids:/collector\n",
    );
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup cgroup rw,cpu,cpuacct,memory,blkio,pids\n",
            dir.path().join("sys/fs/cgroup").display()
        ),
    );
    for (file, value) in [
        ("cpuacct.usage", "100000"),
        ("cpuacct.stat", "user 10\nsystem 5\n"),
        ("cpu.cfs_quota_us", "150000"),
        ("cpu.cfs_period_us", "100000"),
        ("memory.usage_in_bytes", "4096"),
        ("memory.limit_in_bytes", "8192"),
        ("memory.stat", "total_rss 4096\ntotal_cache 0\n"),
        (
            "blkio.throttle.io_service_bytes_recursive",
            "8:0 Read 100\n",
        ),
        ("pids.current", "3"),
        ("pids.max", "max"),
        (
            "cpu.pressure",
            "some avg10=10 avg60=10 avg300=10 total=100\n",
        ),
        ("io.stat", "8:0 rbytes=100 wbytes=200 rios=3 wios=4\n"),
        ("cpuset.cpus.effective", "0-7"),
    ] {
        write(dir.path(), &format!("sys/fs/cgroup/{file}"), value);
    }
    let selected =
        collect_ancestor_context(&procfs, &sys, 1).expect("unsupported hierarchy is absent");
    assert_eq!(selected.context.cgroup_version, 0);
    assert!(
        selected.cpu.is_none()
            && selected.memory.is_none()
            && selected.io.is_none()
            && selected.pids.is_none()
    );
    assert_eq!(selected.context.effective_cpu_quota_usec, None);
    assert_eq!(selected.context.cpuset_cpus, None);
    assert_eq!(selected.context.effective_memory_max, None);
    let rows = collect_ancestor_rows(&sys, &selected, 1);
    assert!(
        rows.ancestor_cpu.is_empty()
            && rows.ancestor_memory.is_empty()
            && rows.io.is_empty()
            && rows.pids.is_empty()
    );
    assert!(
        collect_ancestor_pressure(&sys, &selected, 1)
            .expect("no pressure")
            .is_empty()
    );
    assert!(charged_ancestor_devices(&sys, &selected).is_empty());
}

#[test]
fn denied_alias_paths_do_not_hide_independently_readable_groups() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    if nix::unistd::Uid::effective().is_root() {
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "cgroup::selected::tests::denied_alias_paths_do_not_hide_independently_readable_groups", "--nocapture"])
            .uid(65534).gid(65534).status().expect("unprivileged permission test");
        assert!(status.success(), "unprivileged test failed");
        return;
    }
    let (dir, procfs, sys) = fixture();
    v2(dir.path(), "/pod/blocked/collector", "/");
    std::fs::create_dir_all(dir.path().join("sys/fs/cgroup/pod/blocked/collector"))
        .expect("own group");
    std::fs::create_dir(dir.path().join("sys/fs/cgroup/alias")).expect("accessible alias");
    let mut text =
        std::fs::read_to_string(dir.path().join("proc/self/mountinfo")).expect("mountinfo");
    writeln!(
        &mut text,
        "41 1 0:30 /pod/blocked {} rw - cgroup2 cgroup rw",
        dir.path().join("sys/fs/cgroup/alias").display()
    )
    .expect("alias binding");
    write(dir.path(), "proc/self/mountinfo", &text);
    let blocked = dir.path().join("sys/fs/cgroup/pod");
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0)).expect("deny traversal");
    let denied =
        std::fs::read_dir(blocked.join("blocked")).expect_err("fixture must deny traversal");
    let selected = collect_ancestor_context(&procfs, &sys, 1);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755))
        .expect("restore fixture");
    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        selected
            .expect("readable root")
            .cpu
            .expect("CPU group")
            .base,
        "fs/cgroup"
    );

    // An unreadable lexical root cannot suppress a separately accessible mount.
    std::fs::create_dir(dir.path().join("sys/fs/cgroup/denied")).expect("denied root");
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n41 1 0:30 /pod/blocked {} rw - cgroup2 cgroup rw\n",
            dir.path().join("sys/fs/cgroup/denied").display(),
            dir.path().join("sys/fs/cgroup/alias").display()
        ),
    );
    let denied_root = dir.path().join("sys/fs/cgroup/denied");
    std::fs::set_permissions(&denied_root, std::fs::Permissions::from_mode(0)).expect("deny root");
    let denied = std::fs::read_dir(&denied_root).expect_err("root is unreadable");
    let selected = collect_ancestor_context(&procfs, &sys, 2);
    std::fs::set_permissions(&denied_root, std::fs::Permissions::from_mode(0o755))
        .expect("restore root");
    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        selected
            .expect("readable alias")
            .cpu
            .expect("CPU group")
            .base,
        "fs/cgroup/alias"
    );
    // The readable winner may have a narrower mount root. Verify aliases in
    // either direction when the broader directory is searchable but unreadable.
    std::fs::create_dir_all(dir.path().join("sys/fs/cgroup/broad/pod/blocked"))
        .expect("broad tree");
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n41 1 0:30 /pod {} rw - cgroup2 cgroup rw\n",
            dir.path().join("sys/fs/cgroup/broad").display(),
            dir.path().join("sys/fs/cgroup/alias").display()
        ),
    );
    let broad = dir.path().join("sys/fs/cgroup/broad");
    let pod = broad.join("pod");
    for path in [&broad, &pod] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o111))
            .expect("searchable only");
    }
    let denied = std::fs::read_dir(&broad).expect_err("broad root is unreadable");
    let selected = collect_ancestor_context(&procfs, &sys, 3);
    for path in [&broad, &pod] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("restore broad root");
    }
    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
    assert!(selected.expect("ambiguous readable aliases").cpu.is_none());
}
