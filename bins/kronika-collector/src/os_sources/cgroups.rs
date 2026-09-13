use super::{Interner, OsSources, cgroup, log_degraded};
#[cfg(test)]
use super::{ProcFs, SysFs};

pub(super) fn record_context_section(
    selected: &cgroup::AncestorContext,
    interner: &mut Interner,
    os: &mut OsSources,
) {
    match crate::cgroup_discovery::context_section(interner, selected) {
        Ok(row) => os.cgroup_context = Some(row),
        Err(error) => log_degraded(1_205_002, "cgroup/context", &error),
    }
}

#[cfg(test)]
fn collect_context_section(
    sys: &SysFs,
    interner: &mut Interner,
    _scope: u8,
    ts: i64,
    fs: &ProcFs,
    os: &mut OsSources,
) {
    let selected =
        cgroup::collect_ancestor_context(fs, sys, ts).unwrap_or_else(|_| cgroup::AncestorContext {
            context: cgroup::CgroupContextRow {
                ts,
                ..cgroup::CgroupContextRow::default()
            },
            ..cgroup::AncestorContext::default()
        });
    record_context_section(&selected, interner, os);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_tick_emits_one_scoped_row_with_interned_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let proc_root = dir.path().join("proc");
        let sys_root = dir.path().join("sys");
        std::fs::create_dir_all(proc_root.join("self")).expect("mkdir proc self");
        std::fs::create_dir_all(sys_root.join("fs/cgroup/workload"))
            .expect("mkdir cgroup workload");
        std::fs::write(proc_root.join("self/cgroup"), "0::/workload\n").expect("write self cgroup");
        std::fs::write(
            sys_root.join("fs/cgroup/cgroup.controllers"),
            "cpu memory io cpuset\n",
        )
        .expect("write controllers");
        std::fs::write(
            sys_root.join("fs/cgroup/workload/cpuset.cpus.effective"),
            "0-1\n",
        )
        .expect("write effective cpuset");
        std::fs::write(
            sys_root.join("fs/cgroup/workload/cpu.stat"),
            "usage_usec 10\nuser_usec 6\nsystem_usec 4\n",
        )
        .expect("write cpu stat");
        std::fs::write(sys_root.join("fs/cgroup/workload/memory.current"), "4096\n")
            .expect("write memory current");
        std::fs::write(
            sys_root.join("fs/cgroup/workload/memory.stat"),
            "anon 100\nfile 200\nkernel 50\nslab 20\n",
        )
        .expect("write memory stat");
        std::fs::write(
            sys_root.join("fs/cgroup/workload/io.stat"),
            "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n",
        )
        .expect("write io stat");
        std::fs::write(
            proc_root.join("self/mountinfo"),
            format!(
                "40 1 0:30 / {} rw - cgroup2 cgroup rw\n",
                sys_root.join("fs/cgroup").display()
            ),
        )
        .expect("write cgroup mount binding");
        let procfs = ProcFs::new(proc_root);
        let sys = SysFs::new(sys_root);
        let mut interner = Interner::new(kronika_format::DictLimits::default());
        let mut os = OsSources::empty();

        collect_context_section(&sys, &mut interner, 3, 7, &procfs, &mut os);

        let row = os.cgroup_context.expect("one context row");

        assert_eq!(row.ts.0, 7);
        assert_eq!(row.cgroup_version, 2);
        assert!(row.cpu_path.is_some());
        assert_eq!(row.cpu_path, row.memory_path);
        assert_eq!(row.io_path, row.cpu_path);
        assert_eq!(row.pids_path, row.cpu_path);
        assert_eq!(row.cpu_identity, row.memory_identity);
        assert_eq!(row.cpu_identity, row.io_identity);
        assert_eq!(row.cpu_identity, row.pids_identity);
        assert_eq!(row.cpu_root, row.memory_root);
        assert_eq!(row.cpu_root, row.io_root);
        assert_eq!(row.cpu_root, row.pids_root);
        assert_eq!(row.cpuset_cpus, None);
        assert!(row.cpu_identity.is_some());
        assert!(row.cpu_root.is_some());
        assert_eq!(row.effective_cpu_quota_usec, None);
        assert_eq!(row.effective_cpu_period_usec, None);
        assert_eq!(row.effective_memory_max, None);
        assert_eq!(row.scope, 4);
    }

    #[test]
    fn unreadable_membership_emits_one_unknown_context_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let proc_root = dir.path().join("proc");
        let sys_root = dir.path().join("sys");
        std::fs::create_dir_all(&proc_root).expect("mkdir proc");
        std::fs::create_dir_all(sys_root.join("fs/cgroup")).expect("mkdir cgroup");
        let procfs = ProcFs::new(proc_root);
        let sys = SysFs::new(sys_root);
        let mut interner = Interner::new(kronika_format::DictLimits::default());
        let mut os = OsSources::empty();

        collect_context_section(&sys, &mut interner, 4, 9, &procfs, &mut os);

        let row = os.cgroup_context.expect("one context row");
        assert_eq!(row.ts.0, 9);
        assert_eq!(row.cgroup_version, 0);
        assert_eq!(row.cpu_path, None);
        assert_eq!(row.memory_path, None);
        assert_eq!(row.io_path, None);
        assert_eq!(row.cpuset_cpus, None);
        assert_eq!(row.effective_cpu_quota_usec, None);
        assert_eq!(row.effective_cpu_period_usec, None);
        assert_eq!(row.effective_memory_max, None);
        assert_eq!(row.scope, 4);
    }
}
