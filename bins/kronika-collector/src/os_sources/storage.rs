//! Disk counters and mounted filesystems.

use std::collections::HashSet;
use std::time::Instant;

use kronika_registry::{Section, os_diskstats::OsDiskstats, os_mountinfo::OsMountinfo};
use kronika_source_os::proc::diskstats;
use kronika_source_os::{
    MountEntry, MountStringIds, ProcFs, SysFs, is_kernel_tree_mount, is_pseudo_filesystem,
    mount_row, parse_dev_pair, parse_mountinfo,
};
use kronika_writer::Interner;

use super::io::{intern_str, log_degraded, read_optional_os_file};
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
    let Some(content) = read_optional_os_file(fs, "diskstats", type_id) else {
        return Vec::new();
    };
    let mut rows = match diskstats::parse(&content) {
        Ok(rows) => rows,
        Err(err) => {
            log_degraded(type_id, "diskstats", &err.0);
            return Vec::new();
        }
    };

    if let Some(kept) = kept {
        rows.retain(|row| kept.contains(&(row.major, row.minor)));
    }

    let built: Vec<OsDiskstats> = rows
        .iter()
        .filter_map(|row| {
            let device = intern_str(interner, type_id, "diskstats", &row.device)?;
            Some(row.to_section(scope, ts, device))
        })
        .collect();
    log_collection_finish(type_id, "procfs", built.len(), started.elapsed());
    built
}

/// Read and parse `/proc/self/mountinfo`, resolving `major == 0` subvolume
/// devices via `/sys`.
pub(super) fn mountinfo_entries(fs: &ProcFs) -> Vec<MountEntry> {
    let type_id = OsMountinfo::CONTRACT.type_id.get();
    let Some(content) = read_optional_os_file(fs, "self/mountinfo", type_id) else {
        return Vec::new();
    };
    let mut entries = parse_mountinfo(&content);
    entries.retain(|entry| {
        !is_pseudo_filesystem(&entry.fstype) && !is_kernel_tree_mount(&entry.mount_point)
    });
    resolve_major_zero(&SysFs::from_env(), &mut entries);
    entries
}

/// Recover the real `(major, minor)` of `major == 0` subvolume mounts (btrfs,
/// ZFS) whose source is a `/dev/` node, by reading `class/block/<name>/dev`.
/// Entries that cannot be resolved keep `major == 0` and are dropped by
/// `device_map`/`container_device_set` downstream.
pub(crate) fn resolve_major_zero(sys: &SysFs, entries: &mut [MountEntry]) {
    for entry in entries.iter_mut().filter(|e| e.major == 0) {
        let Some(name) = entry.source.strip_prefix("/dev/") else {
            continue;
        };
        let rel = format!("class/block/{name}/dev");
        if let Ok(content) = sys.read(&rel)
            && let Some((major, minor)) = parse_dev_pair(&content)
        {
            entry.major = major;
            entry.minor = minor;
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
    let mut rows = Vec::new();
    for (entry, space) in entries.iter().zip(capacities) {
        let (Some(mount_point), Some(root), Some(fstype), Some(source)) = (
            intern_str(interner, type_id, "self/mountinfo", &entry.mount_point),
            intern_str(interner, type_id, "self/mountinfo", &entry.root),
            intern_str(interner, type_id, "self/mountinfo", &entry.fstype),
            intern_str(interner, type_id, "self/mountinfo", &entry.source),
        ) else {
            continue;
        };
        rows.push(mount_row(
            entry,
            space,
            scope,
            ts,
            MountStringIds {
                mount_point,
                root,
                fstype,
                source,
            },
        ));
    }
    log_collection_finish(type_id, "procfs", rows.len(), started.elapsed());
    rows
}
