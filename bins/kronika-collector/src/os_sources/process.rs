use super::{
    DueSet, Instant, Interner, OsCgroupMapping, OsSources, ProcFs, ProcessError,
    ProcessIoCredentials, ProcessIoTarget, ProcessReader, SourceKind, Ts, UserReferences, cgroup,
    intern_str, log_collection_finish, log_count_degraded, log_degraded, process_facts,
};

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "process sections share procfs enumeration, segment user state, and degradation counters"
)]
pub(super) fn collect_process_sections(
    fs: &ProcFs,
    process_io: &mut ProcessIoCredentials,
    interner: &mut Interner,
    users: &mut UserReferences,
    scope: u8,
    ts: i64,
    due: &DueSet,
    mut workload_memberships: Option<&mut cgroup::WorkloadMemberships>,
    os: &mut OsSources,
) {
    let hot_due = due.has(SourceKind::OsProcesses);
    let status_due = due.has(SourceKind::OsProcessStatus);
    let mapping_due = due.has(SourceKind::OsCgroupMapping);
    let cgroup_due = workload_memberships.is_some();
    if !hot_due && !status_due && !mapping_due && !cgroup_due {
        return;
    }

    let hot_type_id = 1_100_001_u32;
    let status_type_id = 1_101_001_u32;
    let mapping_type_id = 1_200_001_u32;
    let started = Instant::now();
    let pids = match fs.pid_dirs() {
        Ok(pids) => pids,
        Err(err) => {
            for type_id in [hot_type_id, status_type_id, mapping_type_id] {
                if (type_id == hot_type_id && hot_due)
                    || (type_id == status_type_id && status_due)
                    || (type_id == mapping_type_id && mapping_due)
                {
                    log_degraded(type_id, "process", &err);
                }
            }
            return;
        }
    };
    process_io.retain_live(&pids);
    let facts = match process_facts(fs) {
        Ok(facts) => facts,
        Err(err) => {
            for type_id in [hot_type_id, status_type_id, mapping_type_id] {
                if (type_id == hot_type_id && hot_due)
                    || (type_id == status_type_id && status_due)
                    || (type_id == mapping_type_id && mapping_due)
                {
                    log_degraded(type_id, "process", &err);
                }
            }
            if let Some(memberships) = workload_memberships {
                let mut reader = ProcessReader::new(fs);
                for pid in pids {
                    if let Some(content) = reader.cgroup_membership(pid) {
                        memberships.observe(content);
                    }
                }
            }
            return;
        }
    };
    let mut skipped = 0_usize;
    let mut io_nulls = 0_usize;
    let mut mapping_nulls = 0_usize;
    let mut reader = ProcessReader::new(fs);
    let mut io_targets = Vec::new();
    for pid in pids {
        let cgroup_path = if mapping_due || cgroup_due {
            let membership = reader.cgroup_membership(pid);
            if let (Some(memberships), Some(membership)) =
                (workload_memberships.as_deref_mut(), membership)
            {
                memberships.observe(membership);
            }
            membership.and_then(kronika_source_os::proc::process::parse_cgroup_path)
        } else {
            None
        };
        let read = match reader.read_without_io(pid, facts, ts, cgroup_path) {
            Ok(read) => read,
            Err(ProcessError::Gone(_)) => {
                process_io.forget(pid);
                continue;
            }
            Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        if hot_due {
            let Some(comm) = intern_str(interner, hot_type_id, "process", &read.hot.comm) else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            let cmdline = read
                .hot
                .cmdline
                .as_deref()
                .and_then(|value| intern_str(interner, hot_type_id, "process", value));
            let row =
                kronika_source_os::proc::process::to_hot_section(&read.hot, scope, comm, cmdline);
            users.observe(scope, row.uid);
            users.observe(scope, row.euid);
            os.processes.push(row);
            io_targets.push(ProcessIoTarget::new(pid, read.hot.uid, read.hot.gid));
        }
        if status_due {
            os.process_status
                .push(kronika_source_os::proc::process::to_status_section(
                    &read.status,
                    scope,
                ));
        }
        if mapping_due {
            if let Some(mapping) = read.cgroup {
                if let Some(cgroup_path) = intern_str(
                    interner,
                    mapping_type_id,
                    "process/cgroup",
                    &mapping.cgroup_path,
                ) {
                    os.cgroup_mapping.push(OsCgroupMapping {
                        ts: Ts(mapping.ts),
                        pid: mapping.pid,
                        starttime: Ts(mapping.starttime),
                        cgroup_path,
                        scope,
                    });
                }
            } else {
                mapping_nulls = mapping_nulls.saturating_add(1);
            }
        }
    }

    if hot_due {
        io_nulls = process_io.read(&mut reader, &io_targets, |index, io| {
            kronika_source_os::proc::process::set_hot_section_io(&mut os.processes[index], io);
        });
    }

    if skipped > 0 {
        for type_id in [hot_type_id, status_type_id, mapping_type_id] {
            if (type_id == hot_type_id && hot_due)
                || (type_id == status_type_id && status_due)
                || (type_id == mapping_type_id && mapping_due)
            {
                log_count_degraded(type_id, "process", "process_skipped", skipped);
            }
        }
    }
    if hot_due && io_nulls > 0 {
        log_count_degraded(
            hot_type_id,
            "process/io",
            "process_io_unavailable",
            io_nulls,
        );
    }
    if mapping_due && mapping_nulls > 0 {
        log_count_degraded(
            mapping_type_id,
            "process/cgroup",
            "process_cgroup_unavailable",
            mapping_nulls,
        );
    }
    if hot_due {
        (os.users, os.pending_users) = users.prepare_rows(interner, scope, ts, std::iter::empty());
        log_collection_finish(hot_type_id, "procfs", os.processes.len(), started.elapsed());
    }
    if status_due {
        log_collection_finish(
            status_type_id,
            "procfs",
            os.process_status.len(),
            started.elapsed(),
        );
    }
    if mapping_due {
        log_collection_finish(
            mapping_type_id,
            "procfs",
            os.cgroup_mapping.len(),
            started.elapsed(),
        );
    }
}
