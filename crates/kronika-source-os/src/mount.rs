//! Parse `/proc/self/mountinfo` and derive disk-attribution helpers.
//!
//! In a Kubernetes pod, `/proc/diskstats` reports the whole node's per-device
//! counters. Charging all of them to the pod would double-count the node. This
//! module keeps only the devices the pod actually mounts for data, and drops
//! the bind-mounted infrastructure files (`/etc/hosts`, service-account
//! secrets, ...) that share the node's root device but carry no pod I/O.
//!
//! Pure string logic: no filesystem or syscall reads. The `/sys/class/block`
//! resolution of `major == 0` subvolume devices (btrfs, ZFS) is a later step.

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
    let mut entries = Vec::new();

    for line in content.lines() {
        // The ` - ` separator divides the fixed head plus optional fields from
        // the tail `fstype source superopts`.
        let Some((head, tail)) = line.split_once(" - ") else {
            continue;
        };

        let head_fields: Vec<&str> = head.split_whitespace().collect();
        // Head layout: mount_id parent_id major:minor root mount_point ...
        let (Some(mount_id), Some(parent_id), Some(dev), Some(root), Some(mount_point)) = (
            head_fields.first(),
            head_fields.get(1),
            head_fields.get(2),
            head_fields.get(3),
            head_fields.get(4),
        ) else {
            continue;
        };
        let (Ok(mount_id), Ok(parent_id)) = (mount_id.parse(), parent_id.parse()) else {
            continue;
        };

        let Some((major_s, minor_s)) = dev.split_once(':') else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major_s.parse::<i32>(), minor_s.parse::<i32>()) else {
            continue;
        };

        let mut tail_fields = tail.split_whitespace();
        let (Some(fstype), Some(source)) = (tail_fields.next(), tail_fields.next()) else {
            continue;
        };

        let root = unescape_mountinfo_field(root);
        let mount_point = unescape_mountinfo_field(mount_point);
        let deleted = mount_point.ends_with(" (deleted)");
        let fstype = unescape_mountinfo_field(fstype);
        let source = unescape_mountinfo_field(source);

        entries.push(MountEntry {
            mount_id,
            parent_id,
            major,
            minor,
            root,
            is_k8s_infra: is_k8s_infra_mount(&mount_point),
            mount_point,
            fstype,
            source,
            deleted,
        });
    }

    entries
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_flags_k8s_infra() {
        let c = "\
30 25 8:1 / /data rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
31 25 8:1 /sub /var/lib/postgresql rw shared:2 - ext4 /dev/sda1 rw\n\
40 25 0:35 / /etc/hosts rw - tmpfs tmpfs rw\n";
        let e = parse_mountinfo(c);
        assert_eq!(e.len(), 3);
        assert_eq!(
            (e[0].major, e[0].minor, e[0].mount_point.as_str()),
            (8, 1, "/data")
        );
        assert_eq!(e[0].fstype, "ext4");
        assert_eq!(e[0].source, "/dev/sda1");
        assert!(!e[0].is_k8s_infra);
        assert!(e[2].is_k8s_infra); // /etc/hosts
    }

    #[test]
    fn decodes_mountinfo_octal_escapes() {
        let c = "\
30 25 8:1 / /data\\040pg rw,relatime shared:1 - ext4 /dev/disk\\040one rw\n";
        let e = parse_mountinfo(c);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].mount_point, "/data pg");
        assert_eq!(e[0].source, "/dev/disk one");
    }

    #[test]
    fn container_set_excludes_infra_only_devices_and_picks_short_path() {
        let c = "\
30 25 8:1 / /data rw - ext4 /dev/sda1 rw\n\
31 25 8:1 / /data/postgres/pgdata rw - ext4 /dev/sda1 rw\n\
40 25 253:0 / /etc/hosts rw - ext4 /dev/dm-0 rw\n";
        let e = parse_mountinfo(c);
        let set = container_device_set(&e);
        assert!(set.contains(&(8, 1))); // has non-infra mount /data
        assert!(!set.contains(&(253, 0))); // only /etc/hosts -> excluded
        let map = device_map(&e);
        assert_eq!(display_path(&map[&(8, 1)]), Some("/data")); // shortest
    }

    #[test]
    fn kernel_tree_mounts_are_masks_not_filesystems() {
        assert!(is_kernel_tree_mount("/proc/kcore"));
        assert!(is_kernel_tree_mount("/sys/firmware"));
        assert!(is_kernel_tree_mount("/sys"));
        assert!(!is_kernel_tree_mount("/sysroot"));
        assert!(!is_kernel_tree_mount("/var/lib/kronika/data"));
    }

    #[test]
    fn skips_line_without_separator() {
        let c = "30 25 8:1 / /data rw,relatime shared:1 ext4 /dev/sda1 rw\n";
        let e = parse_mountinfo(c);
        assert!(e.is_empty());
    }

    #[test]
    fn device_map_drops_major_zero_but_parse_keeps_it() {
        let c = "40 25 0:35 / /etc/hosts rw - tmpfs tmpfs rw\n";
        let e = parse_mountinfo(c);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].major, e[0].minor), (0, 35));
        let map = device_map(&e);
        assert!(!map.contains_key(&(0, 35)));
    }

    #[test]
    fn container_set_excludes_all_infra_device() {
        let c = "\
40 25 253:0 / /etc/hosts rw - ext4 /dev/dm-0 rw\n\
41 25 253:0 / /run/secrets/token rw - ext4 /dev/dm-0 rw\n";
        let e = parse_mountinfo(c);
        let set = container_device_set(&e);
        assert!(!set.contains(&(253, 0)));
        assert!(set.is_empty());
    }

    #[test]
    fn display_path_falls_back_to_shortest_when_all_infra() {
        let paths = vec![
            "/run/secrets/kubernetes.io/serviceaccount".to_owned(),
            "/etc/hosts".to_owned(),
        ];
        assert_eq!(display_path(&paths), Some("/etc/hosts"));
    }

    #[test]
    fn mount_row_maps_space_fields() {
        let entry = MountEntry {
            mount_id: 30,
            parent_id: 20,
            major: 8,
            minor: 1,
            root: "/".to_owned(),
            mount_point: "/data".to_owned(),
            fstype: "ext4".to_owned(),
            source: "/dev/sda1".to_owned(),
            deleted: false,
            is_k8s_infra: false,
        };

        let strings = MountStringIds {
            mount_point: StrId(10),
            root: StrId(11),
            fstype: StrId(20),
            source: StrId(30),
        };
        let row = mount_row(&entry, None, 2, 1_000_000, strings);
        assert_eq!(row.total_bytes, None);
        assert_eq!(row.free_bytes, None);
        assert_eq!(row.major, 8);
        assert_eq!(row.minor, 1);
        assert!(!row.is_k8s_infra);
        assert_eq!(row.scope, 2);
        assert_eq!(row.ts, Ts(1_000_000));
        assert_eq!(row.mount_point, StrId(10));
        assert_eq!(row.root, StrId(11));
        assert_eq!(row.fstype, StrId(20));
        assert_eq!(row.source, StrId(30));

        let space = FsSpace {
            total_bytes: 500_000_000,
            free_bytes: 200_000_000,
            total_inodes: 100_000,
            available_inodes: 40_000,
        };
        let row2 = mount_row(&entry, Some(space), 2, 1_000_000, strings);
        assert_eq!(row2.total_bytes, Some(500_000_000));
        assert_eq!(row2.free_bytes, Some(200_000_000));
        assert_eq!(row2.total_inodes, Some(100_000));
        assert_eq!(row2.available_inodes, Some(40_000));
    }
}
