//! Disk counters and mounted filesystems.

use std::collections::HashSet;
use std::time::Instant;

use kronika_registry::{Section, os_diskstats::OsDiskstats, os_mountinfo::OsMountinfo};
use kronika_source_os::proc::diskstats;
use kronika_source_os::{MountEntry, ProcFs, SysFs};
use kronika_writer::Interner;

use super::io::{intern_str, log_degraded};
use crate::logging::log_collection_finish;

/// Read and parse `/proc/diskstats`, interning device names into rows.
///
/// `/proc/diskstats` reports the whole node. Inside a container the caller
/// passes the devices the pod is charged for and only those rows are kept.
pub(super) fn collect_diskstats(
    fs: &ProcFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    kept: Option<&HashSet<(i32, i32)>>,
) -> Vec<OsDiskstats> {
    let type_id = OsDiskstats::CONTRACT.type_id.get();
    let started = Instant::now();
    let rows = diskstats::collect(fs, scope, ts, kept, |value| {
        intern_str(interner, type_id, "diskstats", value)
    });
    super::io::collected_rows(rows, "diskstats", started)
}

/// Read and parse `/proc/self/mountinfo`, resolving `major == 0` subvolume
/// devices via `/sys`.
pub(super) fn mountinfo_entries(fs: &ProcFs, sys: &SysFs) -> Vec<MountEntry> {
    let type_id = OsMountinfo::CONTRACT.type_id.get();
    match kronika_source_os::mount::collect_entries(fs, sys) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                log_degraded(type_id, "self/mountinfo", &error);
            }
            Vec::new()
        }
    }
}

/// Build one `os_mountinfo` row per parsed mount entry.
///
/// Mount point, fstype, and source strings are interned here. Filesystem
/// capacity uses the filesystem-type allowlist and helper wait budget in
/// [`crate::filesystem_capacity`]. Skipped mounts and missing results have
/// null capacity fields.
pub(crate) fn collect_mountinfo(
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    entries: &[MountEntry],
) -> Vec<OsMountinfo> {
    let type_id = OsMountinfo::CONTRACT.type_id.get();
    let started = Instant::now();
    let capacities = crate::filesystem_capacity::collect(entries);
    let rows = kronika_source_os::mount::to_sections(entries, capacities, scope, ts, |value| {
        intern_str(interner, type_id, "self/mountinfo", value)
    });
    log_collection_finish(type_id, "procfs", rows.len(), started.elapsed());
    rows
}
