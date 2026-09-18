use std::time::Instant;

use kronika_registry::os_cgroup_mapping::OsCgroupMapping;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_process_status::OsProcessStatus;
use kronika_registry::{Section, Ts};
use kronika_source_os::ProcFs;
use kronika_source_os::proc::process::{
    ProcessCgroupRow, ProcessError, ProcessFacts, ProcessHotRow, ProcessIoCredentials,
    ProcessIoTarget, ProcessReader, parse_cgroup_path, process_facts, set_hot_section_io,
    to_hot_section, to_status_section,
};
use kronika_writer::Interner;

use super::io::{intern_str, log_degraded};
use super::{OsSources, SegmentUserNames};
use crate::logging::{log_collection_finish, log_count_degraded};
use crate::scheduler::{DueSet, SourceKind};

const HOT_TYPE_ID: u32 = OsProcess::CONTRACT.type_id.get();
const STATUS_TYPE_ID: u32 = OsProcessStatus::CONTRACT.type_id.get();
const MAPPING_TYPE_ID: u32 = OsCgroupMapping::CONTRACT.type_id.get();

/// Identity and schedule shared by the process sections in one tick.
pub(super) struct ProcessTick<'a> {
    pub(super) scope: u8,
    pub(super) ts: i64,
    pub(super) due: &'a DueSet,
}

impl ProcessTick<'_> {
    fn active_type_ids(&self) -> impl Iterator<Item = u32> {
        [
            (SourceKind::OsProcesses, HOT_TYPE_ID),
            (SourceKind::OsProcessStatus, STATUS_TYPE_ID),
            (SourceKind::OsCgroupMapping, MAPPING_TYPE_ID),
        ]
        .into_iter()
        .filter_map(|(source, type_id)| self.due.has(source).then_some(type_id))
    }
}

#[derive(Default)]
struct ScanStats {
    skipped: usize,
    io_nulls: usize,
    mapping_nulls: usize,
}

/// Scan each PID once, then read I/O counters in batches grouped by credentials.
pub(super) fn collect_process_sections(
    fs: &ProcFs,
    process_io: &mut ProcessIoCredentials,
    interner: &mut Interner,
    users: &mut SegmentUserNames,
    tick: &ProcessTick<'_>,
    os: &mut OsSources,
) {
    let hot_due = tick.due.has(SourceKind::OsProcesses);
    let status_due = tick.due.has(SourceKind::OsProcessStatus);
    let mapping_due = tick.due.has(SourceKind::OsCgroupMapping);
    if !hot_due && !status_due && !mapping_due {
        return;
    }

    let started = Instant::now();
    let (pids, facts) = match prepare_scan(fs, process_io) {
        Ok(scan) => scan,
        Err(error) => {
            for type_id in tick.active_type_ids() {
                log_degraded(type_id, "process", &error);
            }
            return;
        }
    };
    let mut stats = ScanStats::default();
    let mut reader = ProcessReader::new(fs);
    let mut io_targets = Vec::new();
    for pid in pids {
        let cgroup_path = if mapping_due {
            reader.cgroup_membership(pid).and_then(parse_cgroup_path)
        } else {
            None
        };
        let read = match reader.read_without_io(pid, facts, tick.ts, cgroup_path) {
            Ok(read) => read,
            Err(ProcessError::Gone(_)) => {
                process_io.forget(pid);
                continue;
            }
            Err(_) => {
                stats.skipped = stats.skipped.saturating_add(1);
                continue;
            }
        };
        if hot_due {
            let Some(row) = hot_row(&read.hot, interner, tick.scope) else {
                // A required process name failing to intern skips every section for this PID.
                stats.skipped = stats.skipped.saturating_add(1);
                continue;
            };
            users.observe_user(tick.scope, row.uid);
            users.observe_user(tick.scope, row.euid);
            os.processes.push(row);
            io_targets.push(ProcessIoTarget::new(pid, read.hot.uid, read.hot.gid));
        }
        if status_due {
            os.process_status
                .push(to_status_section(&read.status, tick.scope));
        }
        if mapping_due {
            if let Some(mapping) = &read.cgroup {
                if let Some(row) = mapping_row(mapping, interner, tick.scope) {
                    os.cgroup_mapping.push(row);
                }
            } else {
                stats.mapping_nulls = stats.mapping_nulls.saturating_add(1);
            }
        }
    }

    if hot_due {
        stats.io_nulls = process_io.read(&mut reader, &io_targets, |index, io| {
            set_hot_section_io(&mut os.processes[index], io);
        });
    }
    finish_scan(tick, &stats, started, interner, users, os);
}

fn prepare_scan(
    fs: &ProcFs,
    process_io: &mut ProcessIoCredentials,
) -> std::io::Result<(Vec<i32>, ProcessFacts)> {
    let pids = fs.pid_dirs()?;
    process_io.retain_live(&pids);
    let facts = process_facts(fs)?;
    Ok((pids, facts))
}

fn hot_row(hot: &ProcessHotRow, interner: &mut Interner, scope: u8) -> Option<OsProcess> {
    let comm = intern_str(interner, HOT_TYPE_ID, "process", &hot.comm)?;
    let cmdline = hot
        .cmdline
        .as_deref()
        .and_then(|value| intern_str(interner, HOT_TYPE_ID, "process", value));
    Some(to_hot_section(hot, scope, comm, cmdline))
}

fn mapping_row(
    mapping: &ProcessCgroupRow,
    interner: &mut Interner,
    scope: u8,
) -> Option<OsCgroupMapping> {
    let cgroup_path = intern_str(
        interner,
        MAPPING_TYPE_ID,
        "process/cgroup",
        &mapping.cgroup_path,
    )?;
    Some(OsCgroupMapping {
        ts: Ts(mapping.ts),
        pid: mapping.pid,
        starttime: Ts(mapping.starttime),
        cgroup_path,
        scope,
    })
}

fn finish_scan(
    tick: &ProcessTick<'_>,
    stats: &ScanStats,
    started: Instant,
    interner: &mut Interner,
    users: &mut SegmentUserNames,
    os: &mut OsSources,
) {
    if stats.skipped > 0 {
        for type_id in tick.active_type_ids() {
            log_count_degraded(type_id, "process", "process_skipped", stats.skipped);
        }
    }
    if stats.io_nulls > 0 {
        log_count_degraded(
            HOT_TYPE_ID,
            "process/io",
            "process_io_unavailable",
            stats.io_nulls,
        );
    }
    if stats.mapping_nulls > 0 {
        log_count_degraded(
            MAPPING_TYPE_ID,
            "process/cgroup",
            "process_cgroup_unavailable",
            stats.mapping_nulls,
        );
    }
    if tick.due.has(SourceKind::OsProcesses) {
        (os.users, os.pending_users) = users.prepare_rows(interner, tick.ts);
    }
    for (source, type_id, count) in [
        (SourceKind::OsProcesses, HOT_TYPE_ID, os.processes.len()),
        (
            SourceKind::OsProcessStatus,
            STATUS_TYPE_ID,
            os.process_status.len(),
        ),
        (
            SourceKind::OsCgroupMapping,
            MAPPING_TYPE_ID,
            os.cgroup_mapping.len(),
        ),
    ] {
        if tick.due.has(source) {
            log_collection_finish(type_id, "procfs", count, started.elapsed());
        }
    }
}

#[cfg(test)]
#[path = "../tests/os_sources/process.rs"]
mod tests;
