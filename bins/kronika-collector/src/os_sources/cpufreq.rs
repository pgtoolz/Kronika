use super::{
    DueSet, Instant, Interner, OsCpufreq, OsCpufreqPolicy, OsSources, SourceKind, SysFs, Ts,
    intern_str, log_collection_finish, log_degraded,
};
use kronika_source_os::cpufreq;

/// Collect the complete bounded `CPUFreq` policy set once and expose the
/// independently paced reference and temporal sections requested by this tick.
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
            log_degraded(1_122_001, "sysfs/cpufreq", &error);
            return;
        }
    };
    if emit_reference {
        os.cpufreq_policy = observed
            .policies
            .iter()
            .filter_map(|policy| {
                let related_cpus = intern_optional(
                    interner,
                    1_121_001,
                    "sysfs/cpufreq",
                    policy.related_cpus.as_deref(),
                )
                .ok()?;
                let scaling_driver = intern_optional(
                    interner,
                    1_121_001,
                    "sysfs/cpufreq",
                    policy.scaling_driver.as_deref(),
                )
                .ok()?;
                let actual_source = intern_optional(
                    interner,
                    1_121_001,
                    "sysfs/cpufreq",
                    actual_source_name(policy.actual_source),
                )
                .ok()?;
                Some(OsCpufreqPolicy {
                    ts: Ts(ts),
                    policy_id: policy.policy_id,
                    related_cpus,
                    scaling_driver,
                    actual_source,
                    cpuinfo_min_freq_hz: policy.cpuinfo_min_freq_hz,
                    cpuinfo_max_freq_hz: policy.cpuinfo_max_freq_hz,
                    scope,
                })
            })
            .collect();
        if !os.cpufreq_policy.is_empty() {
            log_collection_finish(
                1_121_001,
                "sysfs",
                os.cpufreq_policy.len(),
                started.elapsed(),
            );
        }
    }
    if emit_samples {
        os.cpufreq = observed
            .samples
            .iter()
            .filter_map(|sample| {
                let actual_source = intern_optional(
                    interner,
                    1_122_001,
                    "sysfs/cpufreq",
                    actual_source_name(sample.actual_source),
                )
                .ok()?;
                Some(OsCpufreq {
                    ts: Ts(ts),
                    policy_id: sample.policy_id,
                    actual_source,
                    actual_frequency_hz: sample.actual_frequency_hz,
                    scaling_cur_freq_hz: sample.scaling_cur_freq_hz,
                    scaling_min_freq_hz: sample.scaling_min_freq_hz,
                    scaling_max_freq_hz: sample.scaling_max_freq_hz,
                    online_cpus: sample.online_cpus,
                    scope,
                })
            })
            .collect();
        if !os.cpufreq.is_empty() {
            log_collection_finish(1_122_001, "sysfs", os.cpufreq.len(), started.elapsed());
        }
    }
}

const fn actual_source_name(source: cpufreq::ActualFrequencySource) -> Option<&'static str> {
    match source {
        cpufreq::ActualFrequencySource::Unavailable => None,
        cpufreq::ActualFrequencySource::CpuinfoAverage => Some("cpuinfo_avg_freq"),
        cpufreq::ActualFrequencySource::CpuinfoCurrent => Some("cpuinfo_cur_freq"),
    }
}

fn intern_optional(
    interner: &mut Interner,
    type_id: u32,
    source: &'static str,
    value: Option<&str>,
) -> Result<Option<kronika_registry::StrId>, ()> {
    value.map_or(Ok(None), |value| {
        intern_str(interner, type_id, source, value)
            .map(Some)
            .ok_or(())
    })
}
