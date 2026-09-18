use super::{
    DueSet, Interner, OsSources, ProcFs, ProcessIoCredentials, ProcessTick, SegmentUserNames,
    SourceKind, collect_process_sections,
};
use crate::config::CollectorMode;
use crate::os_sources::{OsTick, collect_os_sources};
use crate::scheduler::{Intervals, Scheduler};
use kronika_format::DictLimits;
use kronika_source_os::{OsScope, PasswdSnapshot, SysFs};
use rustix::fs::inotify;
use std::path::Path;

fn proc_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("proc root");
    std::fs::write(dir.path().join("stat"), "btime 100\n").expect("boot time");
    std::fs::write(
        dir.path().join("passwd"),
        "worker:x:1000:1000::/:/bin/false\nother:x:1001:1001::/:/bin/false\n",
    )
    .expect("passwd fixture");
    dir
}

fn write_process(root: &Path, pid: i32, comm: &str) {
    let process = root.join(pid.to_string());
    std::fs::create_dir_all(&process).expect("process directory");
    std::fs::write(
        process.join("stat"),
        format!(
            "{pid} ({comm}) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 -5 16 17 190 204800 12 21 22 23 24 25 26 27 28 29 30 31 32 33 15 2 7 8 9 10 11 12 13 14 15"
        ),
    )
    .expect("process stat");
    std::fs::write(
        process.join("status"),
        "Uid: 1000 1001 1000 1000\nGid: 1000 1000 1000 1000\nVmData: 42 kB\n",
    )
    .expect("process status");
    std::fs::write(process.join("cgroup"), "0::/group\n").expect("process membership");
    std::fs::write(
        process.join("io"),
        format!("read_bytes: {}\nwrite_bytes: {}\n", pid * 10, pid * 100),
    )
    .expect("process I/O");
}

fn collect(root: &Path, kinds: Vec<SourceKind>, interner: &mut Interner) -> OsSources {
    let fs = ProcFs::new(root.to_path_buf());
    let mut process_io = ProcessIoCredentials::new();
    let passwd = PasswdSnapshot::read(&root.join("passwd")).expect("read fixture users");
    let mut users = SegmentUserNames::with_passwd(passwd);
    let mut os = OsSources::default();
    collect_process_sections(
        &fs,
        &mut process_io,
        interner,
        &mut users,
        &ProcessTick {
            scope: 4,
            ts: 7,
            due: &DueSet::for_test(kinds),
        },
        &mut os,
    );
    os
}

fn all_sections() -> Vec<SourceKind> {
    vec![
        SourceKind::OsProcesses,
        SourceKind::OsProcessStatus,
        SourceKind::OsCgroupMapping,
    ]
}

fn full_dictionary() -> Interner {
    let limits = DictLimits::new(10, 10)
        .expect("valid string limits")
        .with_max_total_bytes(10)
        .expect("two retained strings fit");
    let mut interner = Interner::new(limits);
    interner.intern(b"kept").expect("retained comm");
    interner.intern(b"/group").expect("retained cgroup path");
    interner
}

#[test]
fn process_sections_follow_independent_schedules_and_hot_rows_capture_users() {
    let dir = proc_root();
    write_process(dir.path(), 10, "worker");
    std::fs::write(dir.path().join("10/cmdline"), b"worker\0--flag\0")
        .expect("available command line");
    for (kinds, expected) in [
        (vec![], (0, 0, 0, 0)),
        (vec![SourceKind::OsProcesses], (1, 0, 0, 2)),
        (vec![SourceKind::OsProcessStatus], (0, 1, 0, 0)),
        (vec![SourceKind::OsCgroupMapping], (0, 0, 1, 0)),
        (all_sections(), (1, 1, 1, 2)),
    ] {
        let mut interner = Interner::new(DictLimits::default());
        let os = collect(dir.path(), kinds, &mut interner);

        assert_eq!(
            (
                os.processes.len(),
                os.process_status.len(),
                os.cgroup_mapping.len(),
                os.users.len()
            ),
            expected
        );
        if !os.processes.is_empty() {
            assert!(os.processes[0].cmdline.is_some());
            assert_eq!(
                (
                    os.processes[0].pid,
                    os.processes[0].ts.0,
                    os.processes[0].scope
                ),
                (10, 7, 4)
            );
            assert_eq!(os.pending_users, [(4, 1000), (4, 1001)]);
        }
        if let Some(status) = os.process_status.first() {
            assert_eq!(
                (status.pid, status.vm_data, status.ts.0, status.scope),
                (10, 42, 7, 4)
            );
        }
        if let Some(mapping) = os.cgroup_mapping.first() {
            assert_eq!((mapping.pid, mapping.ts.0, mapping.scope), (10, 7, 4));
        }
    }
}

#[test]
fn skipped_pids_do_not_shift_io_onto_another_process() {
    let dir = proc_root();
    for pid in [10, 11, 13, 14] {
        write_process(dir.path(), pid, "worker");
    }
    std::fs::write(dir.path().join("11/stat"), "malformed").expect("malformed stat");
    std::fs::create_dir(dir.path().join("12")).expect("disappeared process files");
    std::fs::remove_file(dir.path().join("14/io")).expect("unavailable I/O");
    let mut interner = Interner::new(DictLimits::default());

    let os = collect(dir.path(), all_sections(), &mut interner);

    assert_eq!(
        os.processes
            .iter()
            .map(|row| (row.pid, row.read_bytes, row.write_bytes))
            .collect::<Vec<_>>(),
        [
            (10, Some(100), Some(1000)),
            (13, Some(130), Some(1300)),
            (14, None, None)
        ]
    );
    assert_eq!(
        os.process_status
            .iter()
            .map(|row| row.pid)
            .collect::<Vec<_>>(),
        [10, 13, 14]
    );
    assert_eq!(
        os.cgroup_mapping
            .iter()
            .map(|row| row.pid)
            .collect::<Vec<_>>(),
        [10, 13, 14]
    );
}

#[test]
fn required_comm_failure_skips_all_due_sections_but_not_later_pids() {
    let dir = proc_root();
    write_process(dir.path(), 10, "new");
    write_process(dir.path(), 11, "kept");
    let mut interner = full_dictionary();

    let os = collect(dir.path(), all_sections(), &mut interner);

    assert_eq!(
        os.processes
            .iter()
            .map(|row| (row.pid, row.read_bytes))
            .collect::<Vec<_>>(),
        [(11, Some(110))]
    );
    assert_eq!(
        os.process_status
            .iter()
            .map(|row| row.pid)
            .collect::<Vec<_>>(),
        [11]
    );
    assert_eq!(
        os.cgroup_mapping
            .iter()
            .map(|row| row.pid)
            .collect::<Vec<_>>(),
        [11]
    );
}

#[test]
fn optional_string_failures_affect_only_the_command_line_or_mapping() {
    for (file, content, mapping_pids) in [
        ("cmdline", "new\0argument\0", &[10, 11][..]),
        ("cgroup", "0::/uncached\n", &[11][..]),
    ] {
        let dir = proc_root();
        write_process(dir.path(), 10, "kept");
        write_process(dir.path(), 11, "kept");
        std::fs::write(dir.path().join("10").join(file), content).expect("uncached string");
        let mut interner = full_dictionary();

        let os = collect(dir.path(), all_sections(), &mut interner);

        assert_eq!(
            os.processes.iter().map(|row| row.pid).collect::<Vec<_>>(),
            [10, 11]
        );
        assert_eq!(os.processes[0].cmdline, None);
        assert_eq!(
            os.process_status
                .iter()
                .map(|row| row.pid)
                .collect::<Vec<_>>(),
            [10, 11]
        );
        assert_eq!(
            os.cgroup_mapping
                .iter()
                .map(|row| row.pid)
                .collect::<Vec<_>>(),
            mapping_pids
        );
    }
}

#[test]
fn disabled_cgroups_preserve_os_and_processes_without_reads_or_warnings() {
    const CHILD: &str = "KRONIKA_DISABLED_CGROUP_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "os_sources::process::tests::disabled_cgroups_preserve_os_and_processes_without_reads_or_warnings", "--nocapture"])
            .env(CHILD, "1")
            .output().expect("isolated test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("cgroup"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    disabled_cgroup_scenario();
}

fn disabled_cgroup_scenario() {
    for (in_container, version) in [(false, "cgroup"), (false, "cgroup2"), (true, "cgroup")] {
        let dir = proc_root();
        write_process(dir.path(), 10, "worker");
        std::fs::create_dir_all(dir.path().join("self")).expect("self");
        std::fs::create_dir_all(dir.path().join("pressure")).expect("pressure");
        std::fs::write(
            dir.path().join("stat"),
            "cpu 10 0 10 80 0 0 0 0 0 0\nbtime 100\n",
        )
        .expect("CPU");
        std::fs::write(
            dir.path().join("meminfo"),
            "MemTotal: 1024 kB\nMemFree: 512 kB\n",
        )
        .expect("memory");
        std::fs::write(
            dir.path().join("pressure/cpu"),
            "some avg10=0.10 avg60=0.05 avg300=0.02 total=10000\n",
        )
        .expect("PSI");
        std::fs::write(dir.path().join("self/cgroup"), "0::/worker\n").expect("self membership");
        std::fs::write(
            dir.path().join("self/mountinfo"),
            format!("40 1 0:30 / /sys/fs/cgroup rw - {version} cgroup rw\n"),
        )
        .expect("mountinfo");
        let fs = ProcFs::new(dir.path().to_path_buf());
        let admitted = crate::cgroup_discovery::enabled(&fs, CollectorMode::Local, in_container);
        assert!(!admitted);
        let notify = inotify::init(inotify::CreateFlags::NONBLOCK).expect("inotify");
        for name in ["10/cgroup", "self/cgroup"] {
            inotify::add_watch(&notify, dir.path().join(name), inotify::WatchFlags::OPEN)
                .expect("watch membership");
        }
        let mut scheduler = Scheduler::new(Intervals::default(), CollectorMode::Local, admitted);
        let mut interner = Interner::new(DictLimits::default());
        let mut users = SegmentUserNames::default();
        let mut process_io = ProcessIoCredentials::new();
        let start = std::time::Instant::now();
        for (seconds, forced) in [(0, false), (1, true), (60, false)] {
            let now = start + std::time::Duration::from_secs(seconds);
            let due = scheduler.plan(now, forced);
            scheduler.mark_segment_opened();
            let recollection = scheduler.recollection_due(&due, now);
            for due in [&due, &recollection] {
                assert!(!due.has(SourceKind::OsCgroup));
                assert!(!due.has(SourceKind::OsCgroupMapping));
                let os = collect_os_sources(
                    &fs,
                    &SysFs::new(dir.path().join("sys")),
                    &mut process_io,
                    &mut interner,
                    &mut users,
                    &OsTick {
                        scope: 0,
                        ts: 7,
                        in_container,
                        collect_cgroups: admitted,
                        due,
                        cgroup_pass: None,
                    },
                );
                assert!(os.cgroup_context.is_none());
                assert!(os.cgroup_mapping.is_empty());
                assert_eq!(os.processes.len(), 1);
                assert_eq!(os.process_status.len(), 1);
                assert_eq!(
                    (os.processes[0].read_bytes, os.processes[0].write_bytes),
                    (Some(100), Some(1000))
                );
                assert!(os.processes[0].starttime.0 > 0);
                assert_eq!((os.processes[0].uid, os.processes[0].euid), (1000, 1001));
                assert_eq!(
                    os.processes[0].scope,
                    if in_container {
                        OsScope::Container
                    } else {
                        OsScope::Host
                    }
                    .as_u8()
                );
                assert!(!os.cpu.is_empty());
                assert!(os.meminfo.is_some());
                assert_eq!(os.psi.len(), usize::from(!in_container));
                if !in_container {
                    assert_eq!(os.psi[0].some_total, 10000);
                }
            }
        }
        let mut buf = [std::mem::MaybeUninit::uninit(); 512];
        let mut events = inotify::Reader::new(&notify, &mut buf);
        assert!(
            matches!(events.next(), Err(rustix::io::Errno::AGAIN)),
            "membership file was opened"
        );
    }
}
