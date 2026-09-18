use super::*;
use std::fmt::Write as _;
use std::path::Path;

fn fixture() -> (tempfile::TempDir, ProcFs, SysFs) {
    let dir = tempfile::tempdir().expect("tree");
    std::fs::create_dir_all(dir.path().join("proc/self")).expect("proc");
    std::fs::create_dir_all(dir.path().join("sys/fs/cgroup")).expect("sys");
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw,memory_localevents,pids_localevents\n",
            dir.path().join("sys/fs/cgroup").display()
        ),
    );
    let procfs = ProcFs::new(dir.path().join("proc"));
    let sys = SysFs::new(dir.path().join("sys"));
    (dir, procfs, sys)
}

fn write(root: &Path, path: &str, contents: &str) {
    let target = root.join(path);
    std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
    std::fs::write(target, contents).expect("write");
}

#[test]
fn empty_threaded_and_no_pid_directories_keep_independent_optional_fields() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "sys/fs/cgroup/memory.stat",
        "anon 11\nfile 12\nkernel 13\nslab 14\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/empty/cgroup.events",
        "populated 0\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/empty/threaded/cgroup.type",
        "threaded\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/empty/threaded/cpu.max",
        "150000 100000\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/empty/threaded/cpuset.cpus.effective",
        "0-7\n",
    );
    let mut groups = Vec::new();
    let stats = walk_visible_v2(&procfs, &sys, 5, |row| {
        if let DiscoveryRow::Group(group) = row {
            groups.push(group.clone());
        }
        Ok(())
    })
    .expect("walk");
    assert_eq!(stats.groups, 3);
    assert_eq!(stats.metric_files_read, 3);
    let root = groups
        .iter()
        .find(|group| group.cgroup_path == "/")
        .expect("root");
    assert_eq!(root.memory.current, None);
    assert_eq!(root.memory.anon, Some(11));
    assert!(root.memory_localevents && root.pids_localevents);
    let child = groups
        .iter()
        .find(|group| group.cgroup_path == "/empty/threaded")
        .expect("threaded");
    assert_eq!(child.cpu.quota_usec, Some(150_000));
    assert_eq!(child.cpu.cpuset_cpus, Some(8));
    assert!(child.parent_identity.is_some());
}

#[test]
fn large_hierarchy_and_device_list_have_no_old_global_ceiling() {
    let (dir, procfs, sys) = fixture();
    for n in 0..600 {
        std::fs::create_dir(dir.path().join(format!("sys/fs/cgroup/group-{n}"))).expect("group");
    }
    let mut input = String::new();
    for minor in 0..1100 {
        writeln!(input, "8:{minor} rbytes={minor} wbytes=2 rios=3 wios=4").expect("line");
    }
    write(dir.path(), "sys/fs/cgroup/group-0/io.stat", &input);
    let mut io_count = 0;
    let stats = walk_visible_v2(&procfs, &sys, 6, |row| {
        if let DiscoveryRow::Io(io) = row {
            assert_eq!(io.rbytes, Some(i64::from(io.minor)));
            io_count += 1;
        }
        Ok(())
    })
    .expect("walk");
    assert_eq!(stats.groups, 601);
    assert_eq!(stats.io_rows, 1100);
    assert_eq!(io_count, 1100);
    assert_eq!(stats.metric_files_read, 1);
}

#[test]
fn aliases_deduplicate_objects_and_symlinks_do_not_escape_or_loop() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "sys/fs/cgroup/pod/cpu.stat", "usage_usec 0\n");
    std::os::unix::fs::symlink(".", dir.path().join("sys/fs/cgroup/loop")).expect("loop");
    std::os::unix::fs::symlink(dir.path(), dir.path().join("sys/fs/cgroup/escape"))
        .expect("escape");
    let mountinfo = format!(
        "40 1 0:30 / {} rw - cgroup2 cgroup rw\n41 1 0:30 /pod {} rw - cgroup2 cgroup rw\n",
        dir.path().join("sys/fs/cgroup").display(),
        dir.path().join("sys/fs/cgroup/pod").display()
    );
    write(dir.path(), "proc/self/mountinfo", &mountinfo);
    let stats = walk_visible_v2(&procfs, &sys, 7, |_| Ok(())).expect("walk");
    assert_eq!(stats.groups, 2);
    assert_eq!(stats.metric_files_read, 1);
}

#[test]
fn unlimited_and_events_sources_remain_distinct_from_missing_values() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "sys/fs/cgroup/memory.max", "max\n");
    write(dir.path(), "sys/fs/cgroup/memory.high", "broken\n");
    write(
        dir.path(),
        "sys/fs/cgroup/memory.events",
        "high 90\nmax 91\noom 92\noom_kill 93\n",
    );
    write(
        dir.path(),
        "sys/fs/cgroup/memory.events.local",
        "high 1\nmax 2\noom 3\noom_kill 4\noom_group_kill 5\n",
    );
    write(dir.path(), "sys/fs/cgroup/pids.events.local", "max 8\n");
    write(dir.path(), "sys/fs/cgroup/pids.events", "max 999\n");
    let mut group = None;
    let stats = walk_visible_v2(&procfs, &sys, 8, |row| {
        if let DiscoveryRow::Group(row) = row {
            group = Some(row.clone());
        }
        Ok(())
    })
    .expect("walk");
    let group = group.expect("root");
    assert_eq!(group.memory.max, Some(-1));
    assert_eq!(group.memory.high, None);
    assert_eq!(group.memory.high_events, Some(90));
    assert_eq!(group.memory.local_high_events, Some(1));
    assert_eq!(group.memory.local_oom_group_kill, Some(5));
    assert_eq!(group.pids.failure_max, Some(8));
    assert_eq!(group.pids.events_source, 1);
    assert_eq!(stats.metric_files_read, 5);
}

#[test]
fn opened_directory_identity_survives_path_replacement_during_callback() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "sys/fs/cgroup/pod/io.stat", "8:0 rbytes=10\n");
    let mut original = None;
    walk_visible_v2(&procfs, &sys, 9, |row| {
        match row {
            DiscoveryRow::Group(group) if group.cgroup_path == "/pod" => {
                original = Some(group.cgroup_identity.clone());
                std::fs::rename(
                    dir.path().join("sys/fs/cgroup/pod"),
                    dir.path().join("retired"),
                )
                .expect("rename");
                write(dir.path(), "sys/fs/cgroup/pod/io.stat", "8:0 rbytes=1000\n");
            }
            DiscoveryRow::Io(io) => {
                assert_eq!(Some(&io.cgroup_identity), original.as_ref());
                assert_eq!(io.rbytes, Some(10));
            }
            DiscoveryRow::Group(_) => {}
        }
        Ok(())
    })
    .expect("first walk");
    let mut next = None;
    walk_visible_v2(&procfs, &sys, 10, |row| {
        if let DiscoveryRow::Group(group) = row
            && group.cgroup_path == "/pod"
        {
            next = Some(group.cgroup_identity.clone());
        }
        Ok(())
    })
    .expect("second walk");
    assert_ne!(original, next);
}

#[test]
fn exposed_mount_outside_sys_root_is_discovered_without_default_cgroup_root() {
    let dir = tempfile::tempdir().expect("tree");
    let point = dir.path().join("host-cgroup");
    std::fs::create_dir_all(&point).expect("mount root");
    write(
        dir.path(),
        "host-cgroup/empty/cgroup.events",
        "populated 0\n",
    );
    write(dir.path(), "host-cgroup/cpu.stat", "usage_usec 4\n");
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 /pod {} rw - cgroup2 cgroup rw\n",
            point.display()
        ),
    );
    let procfs = ProcFs::new(dir.path().join("proc"));
    let sys = SysFs::new(dir.path().join("absent-sys"));
    let mut roots = Vec::new();
    let stats = walk_visible_v2(&procfs, &sys, 11, |row| {
        if let DiscoveryRow::Group(group) = row {
            roots.push(group.mount_root.clone());
        }
        Ok(())
    })
    .expect("external exposed mount");
    assert_eq!(stats.groups, 2);
    assert_eq!(stats.metric_files_read, 1);
    assert_eq!(roots, vec!["/pod", "/pod"]);
}

#[test]
fn nested_exposed_mount_is_a_boundary_not_a_false_parent_link() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "sys/fs/cgroup/a/alias/cpu.stat",
        "usage_usec 7\n",
    );
    write(
        dir.path(),
        "proc/self/mountinfo",
        &format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n41 1 0:30 /other {} rw - cgroup2 cgroup rw\n",
            dir.path().join("sys/fs/cgroup").display(),
            dir.path().join("sys/fs/cgroup/a/alias").display()
        ),
    );
    let mut groups = Vec::new();
    let stats = walk_visible_v2(&procfs, &sys, 12, |row| {
        if let DiscoveryRow::Group(group) = row {
            groups.push(group.clone());
        }
        Ok(())
    })
    .expect("walk nested mount");
    assert_eq!(stats.groups, 3);
    assert!(!groups.iter().any(|group| group.cgroup_path == "/a/alias"));
    let nested = groups
        .iter()
        .find(|group| group.mount_root == "/other")
        .expect("separate exposed root");
    assert_eq!(nested.cgroup_path, "/");
    assert_eq!(nested.parent_identity, None);
    assert_eq!(nested.cpu.usage_usec, Some(7));
}

#[test]
fn malformed_present_values_are_reported_without_losing_readable_peers() {
    let (dir, procfs, sys) = fixture();
    write(
        dir.path(),
        "sys/fs/cgroup/cpu.stat",
        "usage_usec bad\nuser_usec 2\n",
    );
    write(dir.path(), "sys/fs/cgroup/memory.current", "invalid\n");
    write(dir.path(), "sys/fs/cgroup/memory.stat", "anon 6\n");
    write(
        dir.path(),
        "sys/fs/cgroup/io.stat",
        "invalid-device rbytes=1\n8:0 rbytes=bad wbytes=5\n",
    );
    let mut io_rows = 0;
    let stats = walk_visible_v2(&procfs, &sys, 13, |row| {
        match row {
            DiscoveryRow::Group(group) => {
                assert_eq!(group.cpu.usage_usec, None);
                assert_eq!(group.cpu.user_usec, Some(2));
                assert_eq!(group.memory.current, None);
                assert_eq!(group.memory.anon, Some(6));
            }
            DiscoveryRow::Io(io) => {
                assert_eq!(io.rbytes, None);
                assert_eq!(io.wbytes, Some(5));
                io_rows += 1;
            }
        }
        Ok(())
    })
    .expect("partial readings");
    assert_eq!(stats.metric_errors, 4);
    assert!(
        stats
            .first_error
            .as_deref()
            .is_some_and(|error| error.contains("cpu.stat"))
    );
    assert_eq!(io_rows, 1);
}

#[test]
fn primary_limits_reuse_files_during_normal_discovery() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "proc/self/cgroup", "0::/child\n");
    write(dir.path(), "sys/fs/cgroup/cpu.max", "150000 100000\n");
    write(dir.path(), "sys/fs/cgroup/memory.max", "1000\n");
    write(dir.path(), "sys/fs/cgroup/cpuset.cpus.effective", "0-7\n");
    write(dir.path(), "sys/fs/cgroup/child/cpu.max", "max 100000\n");
    let selected = crate::cgroup::select_ancestor_context(&procfs, &sys, 14).expect("select");
    let stats = walk_visible_v2_with_primary(&procfs, &sys, 14, &selected, |_, context| {
        assert_eq!(context.context.effective_cpu_quota_usec, Some(150_000));
        assert_eq!(context.context.effective_memory_max, Some(1000));
        assert_eq!(context.context.cpuset_cpus, Some(8));
        Ok(())
    })
    .expect("walk with primary");
    assert_eq!(stats.groups, 2);
    assert_eq!(stats.metric_files_read, 4);
}

#[test]
fn known_selected_child_survives_non_enumerable_root_with_inherited_limits() {
    use std::os::unix::fs::PermissionsExt as _;
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "proc/self/cgroup", "0::/child\n");
    write(dir.path(), "sys/fs/cgroup/cpu.max", "150000 100000\n");
    write(dir.path(), "sys/fs/cgroup/memory.max", "1000\n");
    write(
        dir.path(),
        "sys/fs/cgroup/child/cpu.stat",
        "usage_usec 3\nuser_usec 2\nsystem_usec 1\n",
    );
    write(dir.path(), "sys/fs/cgroup/child/cpu.max", "max 100000\n");
    write(dir.path(), "sys/fs/cgroup/child/memory.max", "2000\n");
    write(
        dir.path(),
        "sys/fs/cgroup/child/cpuset.cpus.effective",
        "0-3\n",
    );
    let root = dir.path().join("sys/fs/cgroup");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o111))
        .expect("hide directory entries");
    if std::fs::read_dir(&root).is_ok() {
        eprintln!("permission_fixture=SKIPPED reason=DAC_override_allows_directory_listing");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("restore root fixture");
        return;
    }
    eprintln!("permission_fixture=EXECUTED root_listing=denied known_child=readable");
    let selected = crate::cgroup::select_ancestor_context(&procfs, &sys, 15).expect("select child");
    assert_eq!(selected.group.as_ref().expect("child").path, "/child");
    let result = walk_visible_v2_with_primary(&procfs, &sys, 15, &selected, |row, context| {
        assert_eq!(context.context.effective_cpu_quota_usec, Some(150_000));
        assert_eq!(context.context.effective_memory_max, Some(1000));
        if let DiscoveryRow::Group(group) = row {
            assert_eq!(group.cgroup_path, "/child");
            assert_eq!(group.cpu.usage_usec, Some(3));
            assert_eq!(group.cpu.quota_usec, Some(-1));
        }
        Ok(())
    });
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
        .expect("restore root fixture");
    let stats = result.expect("walk inaccessible root");
    assert_eq!(stats.groups, 1);
    assert_eq!(stats.metric_files_read, 6);
}

#[test]
fn primary_charged_devices_are_not_dropped_above_1024_rows() {
    let (dir, procfs, sys) = fixture();
    write(dir.path(), "proc/self/cgroup", "0::/\n");
    let mut io = String::new();
    for minor in 0..1100 {
        writeln!(io, "8:{minor} rbytes=1").expect("line");
    }
    write(dir.path(), "sys/fs/cgroup/io.stat", &io);
    let selected = crate::cgroup::select_ancestor_context(&procfs, &sys, 16).expect("selection");
    let devices = crate::cgroup::charged_ancestor_devices(&sys, &selected);
    assert_eq!(devices.len(), 1100);
    assert_eq!(devices.last(), Some(&(8, 1099)));
}
