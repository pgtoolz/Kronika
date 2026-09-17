//! Admit interrupt and kernel resource rows with collector diagnostics.

use super::OsSources;
use super::io::{intern_str, log_degraded};
use crate::logging::log_collection_finish;
use kronika_registry::{
    Section, os_interrupts::OsInterrupts, os_kernel_limits::OsKernelLimits, os_softirq::OsSoftirq,
};
use kronika_source_os::{CollectionError, ProcFs, kernel};
use kronika_writer::Interner;
use std::time::Instant;

pub(super) fn collect_kernel_metrics(
    fs: &ProcFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    cpu_count: usize,
    os: &mut OsSources,
) {
    let type_id = OsInterrupts::CONTRACT.type_id.get();
    store_rows(&mut os.interrupts, "interrupts", || {
        kernel::collect_interrupts(fs, scope, ts, cpu_count, |value| {
            intern_str(interner, type_id, "interrupts", value)
        })
    });
    let type_id = OsSoftirq::CONTRACT.type_id.get();
    store_rows(&mut os.softirq, "softirqs", || {
        kernel::collect_softirqs(fs, scope, ts, |value| {
            intern_str(interner, type_id, "softirqs", value)
        })
    });
    let started = Instant::now();
    let type_id = OsKernelLimits::CONTRACT.type_id.get();
    if let Some(row) = kernel::collect_limits(fs, scope, ts, |source, error| {
        log_degraded(type_id, source, error);
    }) {
        os.kernel_limits = Some(row);
        log_collection_finish(type_id, "procfs", 1, started.elapsed());
    }
}

fn store_rows<S: Section>(
    output: &mut Vec<S>,
    source: &'static str,
    collect: impl FnOnce() -> Result<Vec<S>, CollectionError>,
) {
    let started = Instant::now();
    let type_id = S::CONTRACT.type_id.get();
    match collect() {
        Ok(rows) => {
            *output = rows;
            log_collection_finish(type_id, "procfs", output.len(), started.elapsed());
        }
        Err(error) if !error.is_missing() => log_degraded(type_id, source, &error),
        Err(_) => {}
    }
}
