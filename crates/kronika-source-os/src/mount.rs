//! Parse `/proc/self/mountinfo` and derive disk-attribution helpers.
//!
//! In a Kubernetes pod, `/proc/diskstats` reports the whole node's per-device
//! counters. Charging all of them to the pod would double-count the node. This
//! module keeps only the devices the pod actually mounts for data, and drops
//! the bind-mounted infrastructure files (`/etc/hosts`, service-account
//! secrets, ...) that share the node's root device but carry no pod I/O.
//!
//! Bounded acquisition resolves `major == 0` subvolume devices through the
//! caller's sysfs root; filesystem capacity remains a caller-supplied observation.

use std::collections::{HashMap, HashSet};

use kronika_registry::StrId;
use kronika_registry::Ts;
use kronika_registry::os_mountinfo::OsMountinfo;

use crate::FsSpace;

/// One `/proc/self/mountinfo` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Mount id in the collector's mount namespace.
    pub mount_id: i32,
    /// Parent mount id in the collector's mount namespace.
    pub parent_id: i32,
    /// Device major number (`0` for pseudo/subvolume filesystems).
    pub major: i32,
    /// Device minor number.
    pub minor: i32,
    /// Filesystem root exposed by this mount (mountinfo field 4).
    pub root: String,
    /// Where the filesystem is mounted (mountinfo field 5), with mountinfo
    /// octal escapes decoded.
    pub mount_point: String,
    /// Filesystem type after the ` - ` separator (e.g. `ext4`, `btrfs`).
    pub fstype: String,
    /// Mount source after the ` - ` separator (e.g. `/dev/sda1`), with
    /// mountinfo octal escapes decoded.
    pub source: String,
    /// Whether the visible mount point has the kernel's deleted suffix.
    pub deleted: bool,
    /// Whether [`mount_point`](Self::mount_point) is a Kubernetes bind-mounted
    /// infrastructure path that shares the node's device but carries no pod I/O.
    pub is_k8s_infra: bool,
}

/// Kubernetes infrastructure mount paths bind-mounted from the node's root
/// disk. In a pod they cause false I/O attribution: `/proc/diskstats` reports
/// the whole node's I/O for the shared device, none of it the pod's.
const K8S_INFRA_MOUNTS: &[&str] = &[
    "/etc/hosts",
    "/etc/hostname",
    "/etc/resolv.conf",
    "/dev/termination-log",
    "/run/secrets/",
    "/var/run/secrets/",
];

/// Kernel interfaces mounted as filesystems. They store nothing, report no
/// capacity, and a host carries dozens of them, which buries the filesystems
/// that hold data — and the device topology built from the same rows.
const PSEUDO_FILESYSTEMS: &[&str] = &[
    "autofs",
    "binfmt_misc",
    "bpf",
    "cgroup",
    "cgroup2",
    "configfs",
    "debugfs",
    "devpts",
    "devtmpfs",
    "efivarfs",
    "fusectl",
    "hugetlbfs",
    "mqueue",
    "nsfs",
    "proc",
    "pstore",
    "ramfs",
    "rpc_pipefs",
    "securityfs",
    "selinuxfs",
    "sysfs",
    "tracefs",
];

/// Whether a filesystem of this type stores data. `tmpfs` and `overlay` do —
/// a full `/dev/shm` or `/run` is a real failure — so they are not listed.
#[must_use]
pub fn is_pseudo_filesystem(fstype: &str) -> bool {
    PSEUDO_FILESYSTEMS.contains(&fstype)
}

/// Whether a mount point lies inside the kernel's `/proc` or `/sys` trees.
/// Container runtimes mask paths there with empty tmpfs; nothing mounted
/// there is a data filesystem.
#[must_use]
pub fn is_kernel_tree_mount(path: &str) -> bool {
    ["/proc", "/sys"]
        .iter()
        .any(|tree| path == *tree || path.starts_with(&format!("{tree}/")))
}

/// Returns `true` if `path` is a Kubernetes infrastructure bind-mount: an exact
/// match or a prefix match against the known infrastructure paths.
#[must_use]
pub fn is_k8s_infra_mount(path: &str) -> bool {
    K8S_INFRA_MOUNTS
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(prefix))
}

/// Parse every `/proc/self/mountinfo` line into a [`MountEntry`].
///
/// Keeps `major == 0` entries; `is_k8s_infra` is computed per mount point.
/// Lines without the ` - ` separator or a required field are skipped.
#[must_use]
pub fn parse_mountinfo(content: &str) -> Vec<MountEntry> {
    content
        .lines()
        .filter_map(|line| {
            // Optional mount fields stop at the separator; none need to be retained.
            let (head, tail) = line.split_once(" - ")?;
            let mut head = head.split_whitespace();
            let mount_id = head.next()?.parse().ok()?;
            let parent_id = head.next()?.parse().ok()?;
            let (major, minor) = head.next()?.split_once(':')?;
            let major = major.parse().ok()?;
            let minor = minor.parse().ok()?;
            let root = unescape_mountinfo_field(head.next()?);
            let mount_point = unescape_mountinfo_field(head.next()?);
            let mut tail = tail.split_whitespace();
            let fstype = unescape_mountinfo_field(tail.next()?);
            let source = unescape_mountinfo_field(tail.next()?);
            Some(MountEntry {
                mount_id,
                parent_id,
                major,
                minor,
                root,
                is_k8s_infra: is_k8s_infra_mount(&mount_point),
                deleted: mount_point.ends_with(" (deleted)"),
                mount_point,
                fstype,
                source,
            })
        })
        .collect()
}

fn unescape_mountinfo_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let (Some(a), Some(b), Some(c)) = (
                octal_digit(bytes[i + 1]),
                octal_digit(bytes[i + 2]),
                octal_digit(bytes[i + 3]),
            )
        {
            let value = u16::from(a) * 64 + u16::from(b) * 8 + u16::from(c);
            if let Ok(byte) = u8::try_from(value) {
                out.push(byte);
                i += 4;
                continue;
            }
        }

        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn octal_digit(byte: u8) -> Option<u8> {
    (b'0'..=b'7').contains(&byte).then(|| byte - b'0')
}

/// Maps `(major, minor)` to its mount points, dropping `major == 0` entries.
#[must_use]
pub fn device_map(entries: &[MountEntry]) -> HashMap<(i32, i32), Vec<String>> {
    let mut map: HashMap<(i32, i32), Vec<String>> = HashMap::new();
    for entry in entries {
        if entry.major == 0 {
            continue;
        }
        map.entry((entry.major, entry.minor))
            .or_default()
            .push(entry.mount_point.clone());
    }
    map
}

/// Real backing devices a pod should be charged for: `(major, minor)` where
/// `major != 0` and at least one mount point is not Kubernetes infrastructure.
#[must_use]
pub fn container_device_set(entries: &[MountEntry]) -> HashSet<(i32, i32)> {
    entries
        .iter()
        .filter(|entry| entry.major != 0 && !entry.is_k8s_infra)
        .map(|entry| (entry.major, entry.minor))
        .collect()
}

/// Picks the path to display for a device: the shortest non-infrastructure
/// path, or the shortest overall when every path is infrastructure.
#[must_use]
pub fn display_path(paths: &[String]) -> Option<&str> {
    paths
        .iter()
        .filter(|p| !is_k8s_infra_mount(p))
        .min_by_key(|p| p.len())
        .or_else(|| paths.iter().min_by_key(|p| p.len()))
        .map(String::as_str)
}

/// Interned string identities used by one mount row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountStringIds {
    /// Interned mount point.
    pub mount_point: StrId,
    /// Interned filesystem root.
    pub root: StrId,
    /// Interned filesystem type.
    pub fstype: StrId,
    /// Interned mount source.
    pub source: StrId,
}

/// Build a registry row for `1_112_002` from a parsed mount entry and optional
/// capacity snapshot.
///
/// The caller interns `entry.mount_point`, `entry.root`, `entry.fstype`, and
/// `entry.source` in `strings`. `space` is `None` when capacity was skipped or
/// unavailable.
#[must_use]
pub fn mount_row(
    entry: &MountEntry,
    space: Option<FsSpace>,
    scope: u8,
    ts: i64,
    strings: MountStringIds,
) -> OsMountinfo {
    OsMountinfo {
        ts: Ts(ts),
        major: entry.major,
        minor: entry.minor,
        mount_point: strings.mount_point,
        root: strings.root,
        fstype: strings.fstype,
        source: strings.source,
        is_k8s_infra: entry.is_k8s_infra,
        total_bytes: space.map(|s| s.total_bytes),
        free_bytes: space.map(|s| s.free_bytes),
        total_inodes: space.map(|s| s.total_inodes),
        available_inodes: space.map(|s| s.available_inodes),
        scope,
    }
}

/// Read mount attribution, filtering kernel/pseudo filesystems before resolving
/// subvolume devices through the supplied sysfs root.
///
/// # Errors
/// Returns a bounded mountinfo read failure.
pub fn collect_entries(fs: &crate::ProcFs, sys: &crate::SysFs) -> std::io::Result<Vec<MountEntry>> {
    let content = fs.read_raw("self/mountinfo")?;
    let mut entries = parse_mountinfo(&content);
    entries.retain(|entry| {
        !is_pseudo_filesystem(&entry.fstype) && !is_kernel_tree_mount(&entry.mount_point)
    });
    resolve_major_zero(sys, &mut entries);
    Ok(entries)
}

/// Recover the real `(major, minor)` of `major == 0` subvolume mounts (btrfs,
/// ZFS) whose source is a `/dev/` node, by reading `class/block/<name>/dev`.
///
/// Entries that cannot be resolved keep `major == 0` and are dropped by
/// `device_map`/`container_device_set` downstream.
pub fn resolve_major_zero(sys: &crate::SysFs, entries: &mut [MountEntry]) {
    for entry in entries.iter_mut().filter(|e| e.major == 0) {
        let Some(name) = entry.source.strip_prefix("/dev/") else {
            continue;
        };
        let rel = format!("class/block/{name}/dev");
        if let Ok(content) = sys.read(&rel)
            && let Some((major, minor)) = crate::parse_dev_pair(&content)
        {
            entry.major = major;
            entry.minor = minor;
        }
    }
}

/// Convert mounts with caller-supplied capacity results and string admission.
/// All four strings are attempted in mount-point/root/type/source order even
/// when one is rejected; incomplete rows are omitted.
#[must_use]
pub fn to_sections(
    entries: &[MountEntry],
    capacities: impl IntoIterator<Item = Option<FsSpace>>,
    scope: u8,
    ts: i64,
    mut intern: impl FnMut(&str) -> Option<StrId>,
) -> Vec<OsMountinfo> {
    let mut rows = Vec::new();
    for (entry, space) in entries.iter().zip(capacities) {
        let (Some(mount_point), Some(root), Some(fstype), Some(source)) = (
            intern(&entry.mount_point),
            intern(&entry.root),
            intern(&entry.fstype),
            intern(&entry.source),
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
    rows
}

#[cfg(test)]
#[path = "tests/mount.rs"]
mod tests;
