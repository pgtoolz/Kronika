//! Type `1_116_001`: kernel-wide handle and cache occupancy from `/proc/sys/fs`.

use crate::{Section, Ts};

/// File handle, inode, and dentry accounting for the whole kernel.
///
/// A database host that runs out of file handles fails in ways that look
/// nothing like a disk or memory problem, so the ceiling is stored next to the
/// usage. Every field is nullable: the three source files are independent and
/// a kernel may not expose all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_116_001,
    name = "os_kernel_limits",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsKernelLimits {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Allocated file handles (`/proc/sys/fs/file-nr` field 1).
    #[column(g, unit = count)]
    pub nr_file: Option<i64>,
    /// Allocated but unused file handles (`file-nr` field 2).
    #[column(g, unit = count)]
    pub nr_free_file: Option<i64>,
    /// System-wide file handle ceiling (`file-nr` field 3).
    #[column(g, unit = count)]
    pub max_file: Option<i64>,
    /// Allocated inodes (`/proc/sys/fs/inode-nr` field 1).
    #[column(g, unit = count)]
    pub nr_inode: Option<i64>,
    /// Free inodes (`inode-nr` field 2).
    #[column(g, unit = count)]
    pub nr_free_inode: Option<i64>,
    /// Allocated dentries (`/proc/sys/fs/dentry-state` field 1).
    #[column(g, unit = count)]
    pub nr_dentry: Option<i64>,
    /// Unused dentries available for reclaim (`dentry-state` field 2).
    #[column(g, unit = count)]
    pub nr_unused_dentry: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_kernel_limits.rs"]
mod tests;
