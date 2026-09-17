//! Bounded interrupt and kernel-resource acquisition with caller-owned string IDs.

use crate::proc::{interrupts, kernel_limits};
use crate::{CollectionError, ProcFs};
use kronika_registry::{
    StrId, Ts, os_interrupts::OsInterrupts, os_kernel_limits::OsKernelLimits, os_softirq::OsSoftirq,
};

/// Read interrupt counters, skipping only rows whose names cannot be admitted.
///
/// # Errors
/// Returns a bounded procfs read failure.
pub fn collect_interrupts(
    fs: &ProcFs,
    scope: u8,
    ts: i64,
    cpu_count: usize,
    mut intern: impl FnMut(&str) -> Option<StrId>,
) -> Result<Vec<OsInterrupts>, CollectionError> {
    let content = fs.read_raw("interrupts")?;
    Ok(interrupts::parse_interrupts(&content, cpu_count)
        .iter()
        .filter_map(|row| {
            Some(OsInterrupts {
                ts: Ts(ts),
                irq: intern(&row.irq)?,
                device: match row.device.as_deref() {
                    Some(text) => Some(intern(text)?),
                    None => None,
                },
                count: row.count,
                scope,
            })
        })
        .collect())
}

/// Read softirq counters in kernel order, with row-local string rejection.
///
/// # Errors
/// Returns a bounded procfs read failure.
pub fn collect_softirqs(
    fs: &ProcFs,
    scope: u8,
    ts: i64,
    mut intern: impl FnMut(&str) -> Option<StrId>,
) -> Result<Vec<OsSoftirq>, CollectionError> {
    let content = fs.read_raw("softirqs")?;
    Ok(interrupts::parse_softirqs(&content)
        .iter()
        .filter_map(|row| {
            Some(OsSoftirq {
                ts: Ts(ts),
                vector: intern(&row.vector)?,
                count: row.count,
                scope,
            })
        })
        .collect())
}

/// Read independent file, inode and dentry facts; unavailable files stay null.
///
/// The observer receives each non-missing read error immediately, without
/// retaining diagnostics or suppressing the other two sources.
#[must_use]
pub fn collect_limits(
    fs: &ProcFs,
    scope: u8,
    ts: i64,
    mut on_error: impl FnMut(&'static str, &std::io::Error),
) -> Option<OsKernelLimits> {
    let mut read = |path| match fs.read_raw(path) {
        Ok(content) => Some(content),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                on_error(path, &error);
            }
            None
        }
    };
    let file_nr = read("sys/fs/file-nr");
    let inode_nr = read("sys/fs/inode-nr");
    let dentry_state = read("sys/fs/dentry-state");
    if file_nr.is_none() && inode_nr.is_none() && dentry_state.is_none() {
        return None;
    }
    let row = kernel_limits::parse_kernel_limits(
        file_nr.as_deref(),
        inode_nr.as_deref(),
        dentry_state.as_deref(),
    );
    Some(OsKernelLimits {
        ts: Ts(ts),
        nr_file: row.nr_file,
        nr_free_file: row.nr_free_file,
        max_file: row.max_file,
        nr_inode: row.nr_inode,
        nr_free_inode: row.nr_free_inode,
        nr_dentry: row.nr_dentry,
        nr_unused_dentry: row.nr_unused_dentry,
        scope,
    })
}

#[cfg(test)]
#[path = "tests/kernel.rs"]
mod tests;
