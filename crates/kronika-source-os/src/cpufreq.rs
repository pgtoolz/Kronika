//! Bounded Linux `CPUFreq` policy discovery and temporal sampling.

use std::collections::BTreeSet;
use std::fmt;

use crate::SysFs;
use kronika_registry::{
    StrId, Ts,
    os_cpufreq::{OsCpufreq, OsCpufreqPolicy},
};

/// Maximum `CPUFreq` policies accepted in one complete collection.
pub const MAX_CPUFREQ_POLICIES: usize = 512;
const MAX_CPU_IDS: usize = 4096;
const POLICY_ROOT: &str = "devices/system/cpu/cpufreq";

/// Kernel attribute chosen for a policy's hardware-derived frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActualFrequencySource {
    /// Neither hardware-derived attribute exists.
    Unavailable = 0,
    /// `cpuinfo_avg_freq`, preferred when the kernel exposes it.
    CpuinfoAverage = 1,
    /// `cpuinfo_cur_freq`, used when the average attribute cannot be read.
    CpuinfoCurrent = 2,
}

impl ActualFrequencySource {
    /// Sysfs attribute used for this frequency, or `None` when unavailable.
    #[must_use]
    pub const fn attribute_name(self) -> Option<&'static str> {
        match self {
            Self::Unavailable => None,
            Self::CpuinfoAverage => Some("cpuinfo_avg_freq"),
            Self::CpuinfoCurrent => Some("cpuinfo_cur_freq"),
        }
    }
}

/// Static reference for one kernel `CPUFreq` policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuFreqPolicy {
    /// Numeric suffix of `policyX`.
    pub policy_id: i32,
    /// Exact `related_cpus` kernel text.
    pub related_cpus: Option<String>,
    /// Scaling driver name.
    pub scaling_driver: Option<String>,
    /// Attribute successfully read for this observation.
    pub actual_source: ActualFrequencySource,
    /// Hardware minimum frequency, hertz.
    pub cpuinfo_min_freq_hz: Option<i64>,
    /// Hardware maximum frequency, hertz.
    pub cpuinfo_max_freq_hz: Option<i64>,
}

/// Temporal sample for one kernel `CPUFreq` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFreqSample {
    /// Numeric suffix of `policyX`.
    pub policy_id: i32,
    /// Attribute successfully read for this observation.
    pub actual_source: ActualFrequencySource,
    /// Hardware-derived frequency, hertz.
    pub actual_frequency_hz: Option<i64>,
    /// `CPUFreq`'s separately reported/requested policy frequency, hertz.
    pub scaling_cur_freq_hz: Option<i64>,
    /// Current lower bound, hertz.
    pub scaling_min_freq_hz: Option<i64>,
    /// Current upper bound, hertz.
    pub scaling_max_freq_hz: Option<i64>,
    /// Online logical CPUs currently covered by the policy.
    pub online_cpus: Option<i32>,
}

/// Complete bounded `CPUFreq` observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuFreqCollection {
    /// Policy references.
    pub policies: Vec<CpuFreqPolicy>,
    /// Policy samples at the same observation time.
    pub samples: Vec<CpuFreqSample>,
}

/// A `CPUFreq` directory exceeded the accepted complete policy set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuFreqError {
    policies: usize,
}

impl fmt::Display for CpuFreqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CPUFreq policy count {} exceeds complete-section ceiling {MAX_CPUFREQ_POLICIES}",
            self.policies
        )
    }
}

impl std::error::Error for CpuFreqError {}

/// Read every policy as one complete bounded observation.
///
/// A missing `CPUFreq` root is ordinary unavailability and returns empty sets.
/// Each observation uses the first successfully parsed actual-frequency source.
///
/// # Errors
/// Returns an error when the policy ceiling is exceeded.
pub fn collect(
    sys: &SysFs,
    include_reference: bool,
    include_samples: bool,
) -> Result<CpuFreqCollection, CpuFreqError> {
    let Ok(entries) = sys.read_dir(POLICY_ROOT) else {
        return Ok(CpuFreqCollection {
            policies: Vec::new(),
            samples: Vec::new(),
        });
    };
    let mut ids = entries
        .iter()
        .filter_map(|entry| policy_id(&entry.name))
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() > MAX_CPUFREQ_POLICIES {
        return Err(CpuFreqError {
            policies: ids.len(),
        });
    }
    let online = include_samples
        .then(|| sys.read("devices/system/cpu/online").ok())
        .flatten()
        .and_then(|value| parse_cpu_list(&value));
    let mut policies = Vec::with_capacity(if include_reference { ids.len() } else { 0 });
    let mut samples = Vec::with_capacity(if include_samples { ids.len() } else { 0 });
    for id in ids {
        let root = format!("{POLICY_ROOT}/policy{id}");
        let related_text = sys.read(&format!("{root}/related_cpus")).ok();
        let related = related_text.as_deref().and_then(parse_cpu_list);
        let (actual_source, actual_frequency_hz) = actual_frequency(sys, &root);
        if include_reference {
            policies.push(CpuFreqPolicy {
                policy_id: id,
                related_cpus: related_text,
                scaling_driver: sys.read(&format!("{root}/scaling_driver")).ok(),
                actual_source,
                cpuinfo_min_freq_hz: read_hz(sys, &root, "cpuinfo_min_freq"),
                cpuinfo_max_freq_hz: read_hz(sys, &root, "cpuinfo_max_freq"),
            });
        }
        if include_samples {
            let affected = sys
                .read(&format!("{root}/affected_cpus"))
                .ok()
                .and_then(|value| parse_cpu_list(&value));
            let online_cpus = affected.as_ref().map(cpu_count).or_else(|| {
                let related = related.as_ref()?;
                let online = online.as_ref()?;
                i32::try_from(related.intersection(online).count()).ok()
            });
            samples.push(CpuFreqSample {
                policy_id: id,
                actual_source,
                actual_frequency_hz,
                scaling_cur_freq_hz: read_hz(sys, &root, "scaling_cur_freq"),
                scaling_min_freq_hz: read_hz(sys, &root, "scaling_min_freq"),
                scaling_max_freq_hz: read_hz(sys, &root, "scaling_max_freq"),
                online_cpus,
            });
        }
    }
    Ok(CpuFreqCollection { policies, samples })
}

fn policy_id(name: &str) -> Option<i32> {
    let id = name.strip_prefix("policy")?.parse::<i32>().ok()?;
    (id >= 0).then_some(id)
}

fn actual_frequency(sys: &SysFs, root: &str) -> (ActualFrequencySource, Option<i64>) {
    for source in [
        ActualFrequencySource::CpuinfoAverage,
        ActualFrequencySource::CpuinfoCurrent,
    ] {
        if let Some(frequency) = source
            .attribute_name()
            .and_then(|name| read_hz(sys, root, name))
        {
            return (source, Some(frequency));
        }
    }
    (ActualFrequencySource::Unavailable, None)
}

fn read_hz(sys: &SysFs, root: &str, name: &str) -> Option<i64> {
    let khz = sys
        .read(&format!("{root}/{name}"))
        .ok()?
        .parse::<i64>()
        .ok()?;
    (khz >= 0).then(|| khz.checked_mul(1000)).flatten()
}

fn cpu_count(cpus: &BTreeSet<i32>) -> i32 {
    i32::try_from(cpus.len()).unwrap_or(i32::MAX)
}

fn parse_cpu_list(content: &str) -> Option<BTreeSet<i32>> {
    let mut cpus = BTreeSet::new();
    for token in content.split(|character: char| character == ',' || character.is_whitespace()) {
        if token.is_empty() {
            continue;
        }
        let (first, last) = token.split_once('-').map_or((token, token), |parts| parts);
        let first = first.parse::<i32>().ok()?;
        let last = last.parse::<i32>().ok()?;
        if first < 0 || last < first {
            return None;
        }
        for cpu in first..=last {
            cpus.insert(cpu);
            if cpus.len() > MAX_CPU_IDS {
                return None;
            }
        }
    }
    (!cpus.is_empty()).then_some(cpus)
}

/// Convert one policy using caller-owned dictionary admission.
///
/// # Errors
/// Returns the first interning failure before attempting later fields.
pub fn policy_row<E>(
    policy: &CpuFreqPolicy,
    intern: &mut impl FnMut(&str) -> Result<StrId, E>,
    scope: u8,
    ts: i64,
) -> Result<OsCpufreqPolicy, E> {
    Ok(OsCpufreqPolicy {
        ts: Ts(ts),
        policy_id: policy.policy_id,
        related_cpus: intern_optional(intern, policy.related_cpus.as_deref())?,
        scaling_driver: intern_optional(intern, policy.scaling_driver.as_deref())?,
        actual_source: intern_optional(intern, policy.actual_source.attribute_name())?,
        cpuinfo_min_freq_hz: policy.cpuinfo_min_freq_hz,
        cpuinfo_max_freq_hz: policy.cpuinfo_max_freq_hz,
        scope,
    })
}

/// Convert one sample, retaining nullable hardware readings.
///
/// # Errors
/// Returns the interning failure for the selected source name.
pub fn sample_row<E>(
    sample: &CpuFreqSample,
    intern: &mut impl FnMut(&str) -> Result<StrId, E>,
    scope: u8,
    ts: i64,
) -> Result<OsCpufreq, E> {
    Ok(OsCpufreq {
        ts: Ts(ts),
        policy_id: sample.policy_id,
        actual_source: intern_optional(intern, sample.actual_source.attribute_name())?,
        actual_frequency_hz: sample.actual_frequency_hz,
        scaling_cur_freq_hz: sample.scaling_cur_freq_hz,
        scaling_min_freq_hz: sample.scaling_min_freq_hz,
        scaling_max_freq_hz: sample.scaling_max_freq_hz,
        online_cpus: sample.online_cpus,
        scope,
    })
}

/// Missing values remain NULL; dictionary failures propagate to skip the row.
fn intern_optional<E>(
    intern: &mut impl FnMut(&str) -> Result<StrId, E>,
    value: Option<&str>,
) -> Result<Option<StrId>, E> {
    value.map(intern).transpose()
}

#[cfg(test)]
#[path = "tests/cpufreq.rs"]
mod tests;
