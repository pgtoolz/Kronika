//! Interrupt counters and kernel resource limits.

use kronika_registry::{
    Section, Ts, os_interrupts::OsInterrupts, os_kernel_limits::OsKernelLimits,
    os_softirq::OsSoftirq,
};
use kronika_source_os::ProcFs;
use kronika_source_os::proc::{interrupts, kernel_limits};
use kronika_writer::Interner;
use std::time::Instant;

use super::OsSources;
use super::io::{intern_str, read_optional_os_file};
use crate::logging::log_collection_finish;

/// Collect interrupts, softirqs, and kernel file/inode/dentry counters.
pub(super) fn collect_kernel_metrics(
    fs: &ProcFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    cpu_count: usize,
    os: &mut OsSources,
) {
    let irq_type_id = OsInterrupts::CONTRACT.type_id.get();
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "interrupts", irq_type_id) {
        let parsed = interrupts::parse_interrupts(&content, cpu_count);
        os.interrupts = parsed
            .iter()
            .filter_map(|row| {
                Some(OsInterrupts {
                    ts: Ts(ts),
                    irq: intern_str(interner, irq_type_id, "interrupts", &row.irq)?,
                    device: match row.device.as_deref() {
                        Some(text) => Some(intern_str(interner, irq_type_id, "interrupts", text)?),
                        None => None,
                    },
                    count: row.count,
                    scope,
                })
            })
            .collect();
        log_collection_finish(
            irq_type_id,
            "procfs",
            os.interrupts.len(),
            started.elapsed(),
        );
    }

    let softirq_type_id = OsSoftirq::CONTRACT.type_id.get();
    let started = Instant::now();
    if let Some(content) = read_optional_os_file(fs, "softirqs", softirq_type_id) {
        os.softirq = interrupts::parse_softirqs(&content)
            .iter()
            .filter_map(|row| {
                Some(OsSoftirq {
                    ts: Ts(ts),
                    vector: intern_str(interner, softirq_type_id, "softirqs", &row.vector)?,
                    count: row.count,
                    scope,
                })
            })
            .collect();
        log_collection_finish(
            softirq_type_id,
            "procfs",
            os.softirq.len(),
            started.elapsed(),
        );
    }

    collect_limits(fs, scope, ts, os);
}

fn collect_limits(fs: &ProcFs, scope: u8, ts: i64, os: &mut OsSources) {
    let limits_type_id = OsKernelLimits::CONTRACT.type_id.get();
    let started = Instant::now();
    let file_nr = read_optional_os_file(fs, "sys/fs/file-nr", limits_type_id);
    let inode_nr = read_optional_os_file(fs, "sys/fs/inode-nr", limits_type_id);
    let dentry_state = read_optional_os_file(fs, "sys/fs/dentry-state", limits_type_id);
    if file_nr.is_some() || inode_nr.is_some() || dentry_state.is_some() {
        let row = kernel_limits::parse_kernel_limits(
            file_nr.as_deref(),
            inode_nr.as_deref(),
            dentry_state.as_deref(),
        );
        os.kernel_limits = Some(OsKernelLimits {
            ts: Ts(ts),
            nr_file: row.nr_file,
            nr_free_file: row.nr_free_file,
            max_file: row.max_file,
            nr_inode: row.nr_inode,
            nr_free_inode: row.nr_free_inode,
            nr_dentry: row.nr_dentry,
            nr_unused_dentry: row.nr_unused_dentry,
            scope,
        });
        log_collection_finish(limits_type_id, "procfs", 1, started.elapsed());
    }
}
