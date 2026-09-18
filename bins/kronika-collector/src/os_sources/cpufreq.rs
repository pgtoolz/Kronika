use std::time::Instant;

use kronika_format::DictError;
use kronika_registry::os_cpufreq::OsCpufreq;
use kronika_registry::{Section, StrId};
use kronika_source_os::{SysFs, cpufreq};
use kronika_writer::Interner;

use super::OsSources;
use super::io::log_degraded;
use crate::logging::log_collection_finish;
use crate::scheduler::{DueSet, SourceKind};

/// Read `CPUFreq` once, emitting policies and measurements on their own schedules.
pub(super) fn collect_cpufreq(
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    due: &DueSet,
    os: &mut OsSources,
) {
    let emit_reference = due.has(SourceKind::OsMountTopo);
    let emit_samples = due.has(SourceKind::OsCore);
    if !emit_reference && !emit_samples {
        return;
    }
    let started = Instant::now();
    let observed = match cpufreq::collect(sys, emit_reference, emit_samples) {
        Ok(observed) => observed,
        Err(error) => {
            log_degraded(OsCpufreq::CONTRACT.type_id.get(), "sysfs/cpufreq", &error);
            return;
        }
    };
    if emit_reference {
        store_rows(
            &mut os.cpufreq_policy,
            observed.policies.iter().map(|policy| {
                cpufreq::policy_row(
                    policy,
                    &mut |value| interner.intern(value.as_bytes()).map(|id| StrId(id.get())),
                    scope,
                    ts,
                )
            }),
            started,
        );
    }
    if emit_samples {
        store_rows(
            &mut os.cpufreq,
            observed.samples.iter().map(|sample| {
                cpufreq::sample_row(
                    sample,
                    &mut |value| interner.intern(value.as_bytes()).map(|id| StrId(id.get())),
                    scope,
                    ts,
                )
            }),
            started,
        );
    }
}

/// Replace a section's rows, logging and skipping any that cannot enter the dictionary.
fn store_rows<S: Section>(
    output: &mut Vec<S>,
    rows: impl Iterator<Item = Result<S, DictError>>,
    started: Instant,
) {
    let type_id = S::CONTRACT.type_id.get();
    *output = rows
        .filter_map(|row| match row {
            Ok(row) => Some(row),
            Err(error) => {
                log_degraded(type_id, "sysfs/cpufreq", &error);
                None
            }
        })
        .collect();
    if !output.is_empty() {
        log_collection_finish(type_id, "sysfs", output.len(), started.elapsed());
    }
}

#[cfg(test)]
#[path = "../tests/os_sources/cpufreq.rs"]
mod tests;
