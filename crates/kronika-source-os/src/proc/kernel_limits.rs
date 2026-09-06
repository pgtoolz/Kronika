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
mod tests {
    use super::{KernelLimitsRow, parse_kernel_limits};

    #[test]
    fn reads_every_field_from_the_three_files() {
        let row = parse_kernel_limits(
            Some("3200\t0\t9223372\n"),
            Some("120000\t1000\n"),
            Some("300000\t250000\t45\t0\t0\t0\n"),
        );
        assert_eq!(row.nr_file, Some(3_200));
        assert_eq!(row.nr_free_file, Some(0));
        assert_eq!(row.max_file, Some(9_223_372));
        assert_eq!(row.nr_inode, Some(120_000));
        assert_eq!(row.nr_free_inode, Some(1_000));
        assert_eq!(row.nr_dentry, Some(300_000));
        assert_eq!(row.nr_unused_dentry, Some(250_000));
    }

    #[test]
    fn an_unreadable_file_leaves_its_fields_null() {
        let row = parse_kernel_limits(None, Some("1 2\n"), None);
        assert_eq!(row.nr_file, None);
        assert_eq!(row.max_file, None);
        assert_eq!(row.nr_inode, Some(1));
        assert_eq!(row.nr_dentry, None);
    }

    #[test]
    fn a_truncated_or_garbled_file_yields_nulls_not_zeros() {
        assert_eq!(
            parse_kernel_limits(Some(""), None, None),
            KernelLimitsRow::default()
        );
        let row = parse_kernel_limits(Some("3200 broken 9223372\n"), None, None);
        assert_eq!(row.nr_file, Some(3_200));
        assert_eq!(row.nr_free_file, None);
        assert_eq!(row.max_file, Some(9_223_372));
    }
}
