//! Type `1_112_002`: mount table from `/proc/self/mountinfo`.

use crate::{Section, StrId, Ts};

/// One `/proc/self/mountinfo` entry with optional filesystem capacity.
///
/// Emitted `on_change`; one row per mount point per collection segment.
/// `total_bytes`/`free_bytes` are `None` outside the collector's local
/// filesystem allowlist or when the bounded capacity pass did not complete
/// the mount point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_112_002,
    name = "os_mountinfo",
    semantics = on_change,
    sort_key("major", "minor", "mount_point", "ts"),
    identity("major", "minor", "mount_point")
)]
pub struct OsMountinfo {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Device major number (`0` for pseudo/subvolume filesystems).
    #[column(l)]
    pub major: i32,
    /// Device minor number.
    #[column(l)]
    pub minor: i32,
    /// Mount point path, as a string dictionary reference.
    #[column(l)]
    pub mount_point: StrId,
    /// Filesystem root exposed by this mount (`mountinfo` field 4).
    #[column(l)]
    pub root: StrId,
    /// Filesystem type (e.g. `ext4`, `btrfs`), as a string dictionary reference.
    #[column(l)]
    pub fstype: StrId,
    /// Mount source device path, as a string dictionary reference.
    #[column(l)]
    pub source: StrId,
    /// Whether this is a Kubernetes infrastructure bind-mount.
    #[column(l)]
    pub is_k8s_infra: bool,
    /// Total filesystem capacity in bytes; `None` when skipped or unavailable.
    #[column(g, unit = bytes)]
    pub total_bytes: Option<i64>,
    /// Available bytes for unprivileged writes; `None` when skipped or unavailable.
    #[column(g, unit = bytes)]
    pub free_bytes: Option<i64>,
    /// Total filesystem inode/file-serial count; `None` when unavailable.
    #[column(g, unit = count)]
    pub total_inodes: Option<i64>,
    /// Inodes/file serials available to unprivileged users; `None` when unavailable.
    #[column(g, unit = count)]
    pub available_inodes: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_mountinfo.rs"]
mod tests;
