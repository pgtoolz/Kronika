//! Parse the `/proc/sys/fs` handle and cache counters (`1_116`).

/// Kernel-wide handle and cache occupancy.
///
/// Every field is optional and read from its own file, so an unreadable or
/// absent source leaves that field null instead of failing the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KernelLimitsRow {
    /// Allocated file handles.
    pub nr_file: Option<i64>,
    /// Allocated but unused file handles.
    pub nr_free_file: Option<i64>,
    /// System-wide file handle ceiling.
    pub max_file: Option<i64>,
    /// Allocated inodes.
    pub nr_inode: Option<i64>,
    /// Free inodes.
    pub nr_free_inode: Option<i64>,
    /// Allocated dentries.
    pub nr_dentry: Option<i64>,
    /// Unused dentries available for reclaim.
    pub nr_unused_dentry: Option<i64>,
}

/// Read the `n`th whitespace-separated integer of a one-line procfs file.
fn field(content: &str, index: usize) -> Option<i64> {
    content.split_whitespace().nth(index)?.parse().ok()
}

/// Build a row from the three `/proc/sys/fs` files.
///
/// `file_nr` is `/proc/sys/fs/file-nr`, `inode_nr` is `inode-nr`, and
/// `dentry_state` is `dentry-state`. Pass `None` for a file that could not be
/// read.
#[must_use]
pub fn parse_kernel_limits(
    file_nr: Option<&str>,
    inode_nr: Option<&str>,
    dentry_state: Option<&str>,
) -> KernelLimitsRow {
    KernelLimitsRow {
        nr_file: file_nr.and_then(|c| field(c, 0)),
        nr_free_file: file_nr.and_then(|c| field(c, 1)),
        max_file: file_nr.and_then(|c| field(c, 2)),
        nr_inode: inode_nr.and_then(|c| field(c, 0)),
        nr_free_inode: inode_nr.and_then(|c| field(c, 1)),
        nr_dentry: dentry_state.and_then(|c| field(c, 0)),
        nr_unused_dentry: dentry_state.and_then(|c| field(c, 1)),
    }
}

#[cfg(test)]
#[path = "../tests/proc/kernel_limits.rs"]
mod tests;
