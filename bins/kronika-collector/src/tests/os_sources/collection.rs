use super::core::collect_pressure_rows;
use super::storage::{collect_diskstats, collect_mountinfo};
use super::{OsSources, OsTick, SegmentUserNames, collect_os_sources};
use crate::scheduler::{DueSet, SourceKind};
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_source_os::{MountEntry, ProcFs, SysFs, cgroup};
use kronika_writer::Interner;

const HOST_CPU_PRESSURE: &str = "some avg10=0.10 avg60=0.05 avg300=0.02 total=10000\n";
const CONTAINER_CPU_PRESSURE: &str = "some avg10=0.20 avg60=0.10 avg300=0.04 total=20000\n";

#[test]
fn unreadable_membership_emits_one_unknown_context_row() {
    let dir = tempfile::tempdir().expect("empty proc root");
    let fs = ProcFs::new(dir.path().to_path_buf());
    let mut interner = Interner::new(kronika_format::DictLimits::default());
    let mut users = SegmentUserNames::default();
    let mut process_io = ProcessIoCredentials::new();
    let due = DueSet::for_test(vec![SourceKind::OsCgroup]);

    let mut os = collect_os_sources(
        &fs,
        &SysFs::new(dir.path().join("sys")),
        &mut process_io,
        &mut interner,
        &mut users,
        &OsTick {
            scope: kronika_source_os::OsScope::Container.as_u8(),
            ts: 9,
            in_container: true,
            collect_cgroups: true,
            due: &due,
            cgroup_pass: None,
        },
    );

    let row = os.deduplicate_context(None).expect("one context row");
    assert_eq!(row.ts.0, 9);
    assert_eq!(row.cgroup_version, 0);
    for field in [
        row.cpu_path,
        row.memory_path,
        row.io_path,
        row.pids_path,
        row.cpu_identity,
        row.memory_identity,
        row.io_identity,
        row.pids_identity,
        row.cpu_root,
        row.memory_root,
        row.io_root,
        row.pids_root,
    ] {
        assert_eq!(field, None);
    }
    assert_eq!(row.cpuset_cpus, None);
    assert_eq!(row.effective_cpu_quota_usec, None);
    assert_eq!(row.effective_cpu_period_usec, None);
    assert_eq!(row.effective_memory_max, None);
    assert_eq!(row.scope, kronika_source_os::OsScope::Unknown.as_u8());
}

#[test]
fn supplied_context_survives_process_only_ticks_and_skips_non_os_ticks() {
    let dir = tempfile::tempdir().expect("empty proc root");
    let fs = ProcFs::new(dir.path().to_path_buf());
    let pass = crate::cgroup_discovery::CgroupPass {
        selected: cgroup::AncestorContext {
            context: cgroup::CgroupContextRow {
                ts: 7,
                ..cgroup::CgroupContextRow::default()
            },
            group: None,
        },
        ..crate::cgroup_discovery::CgroupPass::default()
    };

    for (source, expected_ts) in [
        (SourceKind::OsProcesses, Some(7)),
        (SourceKind::OsProcessStatus, Some(7)),
        (SourceKind::OsCgroup, Some(7)),
        (SourceKind::OsCgroupMapping, Some(7)),
        (SourceKind::PgInstance, None),
        (SourceKind::Logs, None),
    ] {
        let mut interner = Interner::new(kronika_format::DictLimits::default());
        let mut users = SegmentUserNames::default();
        let mut process_io = ProcessIoCredentials::new();
        let due = DueSet::for_test(vec![source]);

        let os = collect_os_sources(
            &fs,
            &SysFs::new(dir.path().join("sys")),
            &mut process_io,
            &mut interner,
            &mut users,
            &OsTick {
                scope: kronika_source_os::OsScope::Container.as_u8(),
                ts: 9,
                in_container: true,
                collect_cgroups: true,
                due: &due,
                cgroup_pass: Some(&pass),
            },
        );

        assert_eq!(
            os.cgroup_context.map(|row| row.ts.0),
            expected_ts,
            "context for {source:?} must keep the discovery timestamp"
        );
    }
}

#[test]
fn pressure_collection_keeps_machine_procfs_scope_and_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("pressure")).expect("mkdir host pressure");
    std::fs::create_dir_all(sys_root.join("fs/cgroup/workload")).expect("mkdir cgroup pressure");
    std::fs::write(proc_root.join("pressure/cpu"), HOST_CPU_PRESSURE).expect("write host pressure");
    std::fs::write(sys_root.join("fs/cgroup/cgroup.controllers"), "cpu\n")
        .expect("write unified marker");
    std::fs::write(
        sys_root.join("fs/cgroup/workload/cpu.pressure"),
        CONTAINER_CPU_PRESSURE,
    )
    .expect("write cgroup pressure");
    let mut os = OsSources::default();
    collect_pressure_rows(
        &ProcFs::new(proc_root),
        &SysFs::new(sys_root),
        0,
        7,
        false,
        None,
        &mut os,
    );
    let rows = os.psi;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].scope, 0);
    assert_eq!(rows[0].some_total, 10_000);
}

#[test]
fn pressure_collection_uses_highest_ancestor_and_neutral_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("self")).expect("mkdir proc self");
    std::fs::create_dir_all(proc_root.join("pressure")).expect("mkdir host pressure");
    std::fs::create_dir_all(sys_root.join("fs/cgroup/workload")).expect("mkdir cgroup pressure");
    std::fs::write(proc_root.join("self/cgroup"), "0::/workload\n").expect("write membership");
    std::fs::write(proc_root.join("pressure/cpu"), HOST_CPU_PRESSURE).expect("write host pressure");
    std::fs::write(sys_root.join("fs/cgroup/cgroup.controllers"), "cpu\n")
        .expect("write unified marker");
    std::fs::write(
        sys_root.join("fs/cgroup/workload/cpu.pressure"),
        CONTAINER_CPU_PRESSURE,
    )
    .expect("write cgroup pressure");
    std::fs::write(
        sys_root.join("fs/cgroup/cpu.pressure"),
        "some avg10=0.30 avg60=0.15 avg300=0.06 total=30000\n",
    )
    .expect("write selected root pressure");
    std::fs::write(
        proc_root.join("self/mountinfo"),
        format!(
            "40 1 0:30 / {} rw - cgroup2 cgroup rw\n",
            sys_root.join("fs/cgroup").display()
        ),
    )
    .expect("write cgroup mount binding");
    let fs = ProcFs::new(proc_root);
    let sys = SysFs::new(sys_root);
    let selected = cgroup::collect_ancestor_context(&fs, &sys, 8).ok();
    let mut os = OsSources::default();
    collect_pressure_rows(&fs, &sys, 0, 8, true, selected.as_ref(), &mut os);
    let rows = os.psi;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].scope, 4);
    assert_eq!(rows[0].some_total, 30_000);
}

#[test]
fn collect_mountinfo_emits_every_mount_entry() {
    let entries = vec![
        MountEntry {
            mount_id: 10,
            parent_id: 1,
            major: 8,
            minor: 1,
            root: "/".to_owned(),
            mount_point: "/data".to_owned(),
            fstype: "ext4".to_owned(),
            source: "/dev/sda1".to_owned(),
            deleted: false,
            is_k8s_infra: false,
        },
        MountEntry {
            mount_id: 11,
            parent_id: 10,
            major: 8,
            minor: 1,
            root: "/".to_owned(),
            mount_point: "/data/wal".to_owned(),
            fstype: "ext4".to_owned(),
            source: "/dev/sda1".to_owned(),
            deleted: false,
            is_k8s_infra: false,
        },
    ];
    let mut interner = Interner::new(kronika_format::DictLimits::default());
    let rows = collect_mountinfo(&mut interner, 0, 1_000_000, &entries);

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter().map(|r| (r.major, r.minor)).collect::<Vec<_>>(),
        vec![(8, 1), (8, 1)]
    );
    assert_ne!(rows[0].mount_point, rows[1].mount_point);
}

// Verify that diskstats rows are not emitted on an OsMountTopo-only tick.
#[test]
fn collect_os_sources_no_diskstats_on_mount_topo_only_tick() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path();

    // diskstats: one device (8:1)
    let diskstats_line = "8 1 sda1 1 0 8 2 3 0 24 4 0 6 6\n";
    std::fs::write(proc_root.join("diskstats"), diskstats_line).expect("write diskstats");

    // self/mountinfo: sda1 mounted at /data
    std::fs::create_dir_all(proc_root.join("self")).expect("mkdir self");
    let mountinfo_line = "30 25 8:1 / /data rw - ext4 /dev/sda1 rw\n";
    std::fs::write(proc_root.join("self/mountinfo"), mountinfo_line).expect("write mountinfo");

    let fs = ProcFs::new(proc_root.to_path_buf());
    let mut interner = Interner::new(kronika_format::DictLimits::default());
    let mut users = SegmentUserNames::default();
    let mut process_io = ProcessIoCredentials::new();
    let due = DueSet::for_test(vec![SourceKind::OsMountTopo]);

    let os = collect_os_sources(
        &fs,
        &SysFs::new(dir.path().join("sys")),
        &mut process_io,
        &mut interner,
        &mut users,
        &OsTick {
            scope: 0,
            ts: 0,
            in_container: false,
            collect_cgroups: false,
            due: &due,
            cgroup_pass: None,
        },
    );

    assert!(
        os.diskstats.is_empty(),
        "diskstats must not be emitted on an OsMountTopo-only tick"
    );
    assert!(
        !os.mountinfo.is_empty(),
        "mountinfo rows must still be built"
    );
}

// A container keeps the devices its mounts sit on and the layers its cgroup
// charges; every other node device stays out of its diskstats.
#[test]
fn container_diskstats_keep_mounted_and_charged_devices_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path();
    std::fs::write(
        proc_root.join("diskstats"),
        "252 0 dm-0 1 0 8 2 3 0 24 4 0 6 6\n\
         259 0 nvme0n1 1 0 8 2 3 0 24 4 0 6 6\n\
         8 16 sdb 1 0 8 2 3 0 24 4 0 6 6\n",
    )
    .expect("write diskstats");
    let fs = ProcFs::new(proc_root.to_path_buf());
    let mut interner = Interner::new(kronika_format::DictLimits::default());
    let kept = std::collections::HashSet::from([(252, 0), (259, 0)]);

    let rows = collect_diskstats(&fs, &mut interner, 0, 0, Some(&kept));
    let devices: Vec<(i32, i32)> = rows.iter().map(|row| (row.major, row.minor)).collect();
    assert_eq!(devices, [(252, 0), (259, 0)]);

    let machine = collect_diskstats(&fs, &mut interner, 0, 0, None);
    assert_eq!(machine.len(), 3, "a machine keeps every node device");
}
