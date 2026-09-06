use super::{
    DueSet, Instant, Interner, OsSources, ProcFs, SourceKind, SysFs, cgroup, intern_str,
    log_collection_finish, log_degraded, process_facts,
};

#[allow(
    clippy::too_many_arguments,
    reason = "the cgroup pass shares one tick's roots, interner, due set, process memberships, and output"
)]
pub(super) fn collect_cgroup_sections(
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    fs: &ProcFs,
    due: &DueSet,
    mut process_memberships: cgroup::WorkloadMemberships,
    selected: &cgroup::AncestorContext,
    os: &mut OsSources,
) {
    if !due.has(SourceKind::OsCgroup) {
        return;
    }

    let context_type_id = 1_205_002_u32;
    let cpu_type_id = 1_201_001_u32;
    let memory_type_id = 1_202_001_u32;
    let io_type_id = 1_203_002_u32;
    let pids_type_id = 1_204_001_u32;
    let started = Instant::now();
    let clock_ticks = process_facts(fs).map_or_else(
        |err| {
            log_degraded(cpu_type_id, "cgroup", &err);
            0
        },
        |facts| facts.clock_ticks_per_sec,
    );

    if let Ok(membership) = fs.read_raw("self/cgroup") {
        process_memberships.observe(&membership);
    }
    let mut rows = match process_memberships.collect(sys, ts, clock_ticks) {
        Ok(rows) => rows,
        Err(err) => {
            for (type_id, source) in [
                (cpu_type_id, "cgroup/cpu"),
                (memory_type_id, "cgroup/memory"),
                (io_type_id, "cgroup/io"),
                (pids_type_id, "cgroup/pids"),
            ] {
                log_degraded(type_id, source, &err);
            }
            cgroup::CgroupCollection::default()
        }
    };
    // Keep separately identified workload rows; selected primary rows are read
    // strictly and emitted once with neutral aggregate scope.
    rows.cpu
        .retain(|row| selected.context.cpu_path.as_deref() != Some(row.cgroup_path.as_str()));
    rows.memory
        .retain(|row| selected.context.memory_path.as_deref() != Some(row.cgroup_path.as_str()));
    rows.io
        .retain(|row| selected.context.io_path.as_deref() != Some(row.cgroup_path.as_str()));
    rows.pids.retain(|row| {
        selected
            .pids
            .as_ref()
            .is_none_or(|group| group.path != row.cgroup_path)
    });
    let primary = cgroup::collect_ancestor_rows(sys, selected, ts, clock_ticks);
    for (rows, scope) in [
        (rows, scope),
        (primary, kronika_source_os::OsScope::Unknown.as_u8()),
    ] {
        push_cgroup_rows(&rows, scope, interner, os);
    }
    log_collection_finish(context_type_id, "cgroup/context", 1, started.elapsed());
    log_collection_finish(
        cpu_type_id,
        "cgroup",
        os.cgroup_cpu.len(),
        started.elapsed(),
    );
    log_collection_finish(
        memory_type_id,
        "cgroup",
        os.cgroup_memory.len(),
        started.elapsed(),
    );
    log_collection_finish(io_type_id, "cgroup", os.cgroup_io.len(), started.elapsed());
    log_collection_finish(
        pids_type_id,
        "cgroup",
        os.cgroup_pids.len(),
        started.elapsed(),
    );
}

fn push_cgroup_rows(
    rows: &cgroup::CgroupCollection,
    scope: u8,
    interner: &mut Interner,
    os: &mut OsSources,
) {
    let cpu_type_id = 1_201_001;
    let memory_type_id = 1_202_001;
    let io_type_id = 1_203_002;
    let pids_type_id = 1_204_001;
    if rows.io_omitted {
        log_degraded(
            io_type_id,
            "cgroup/io",
            &std::io::Error::other(format!(
                "cgroup/device row count exceeds {}",
                cgroup::MAX_CGROUP_IO_ROWS
            )),
        );
    }

    for row in &rows.cpu {
        if let Some(cgroup_path) = intern_str(interner, cpu_type_id, "cgroup/cpu", &row.cgroup_path)
        {
            os.cgroup_cpu
                .push(cgroup::to_cpu_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.ancestor_cpu {
        if let Some(path) = intern_str(interner, 1_201_003, "cgroup/cpu", &row.cgroup_path) {
            os.cgroup_ancestor_cpu
                .push(cgroup::to_ancestor_cpu_section(row, path));
        }
    }
    for row in &rows.memory {
        if let Some(cgroup_path) =
            intern_str(interner, memory_type_id, "cgroup/memory", &row.cgroup_path)
        {
            os.cgroup_memory
                .push(cgroup::to_memory_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.ancestor_memory {
        if let Some(path) = intern_str(interner, 1_202_003, "cgroup/memory", &row.cgroup_path) {
            os.cgroup_ancestor_memory
                .push(cgroup::to_ancestor_memory_section(row, path));
        }
    }
    for row in &rows.io {
        if let Some(cgroup_path) = intern_str(interner, io_type_id, "cgroup/io", &row.cgroup_path) {
            os.cgroup_io
                .push(cgroup::to_io_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.pids {
        if let Some(cgroup_path) =
            intern_str(interner, pids_type_id, "cgroup/pids", &row.cgroup_path)
        {
            os.cgroup_pids
                .push(cgroup::to_pids_section(row, scope, cgroup_path));
        }
    }
}

pub(super) fn record_context_section(
    selected: &cgroup::AncestorContext,
    interner: &mut Interner,
    os: &mut OsSources,
) {
    let groups = [
        &selected.cpu,
        &selected.memory,
        &selected.io,
        &selected.pids,
    ];
    let mut paths = [None; 4];
    let mut identities = [None; 4];
    let mut roots = [None; 4];
    for (index, group) in groups.into_iter().enumerate() {
        if let Some(group) = group {
            paths[index] = intern_str(interner, 1_205_002, "cgroup/context", &group.path);
            identities[index] = intern_str(interner, 1_205_002, "cgroup/context", &group.identity);
            roots[index] = intern_str(interner, 1_205_002, "cgroup/context", &group.root);
        }
    }
    os.cgroup_context = Some(cgroup::to_ancestor_context_section(
        selected, paths, identities, roots,
    ));
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
