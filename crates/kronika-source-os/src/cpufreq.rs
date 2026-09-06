//! Bounded Linux `CPUFreq` policy discovery and temporal sampling.

use std::collections::BTreeSet;
use std::fmt;

use crate::SysFs;

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
    for (source, name) in [
        (ActualFrequencySource::CpuinfoAverage, "cpuinfo_avg_freq"),
        (ActualFrequencySource::CpuinfoCurrent, "cpuinfo_cur_freq"),
    ] {
        if let Some(frequency) = read_hz(sys, root, name) {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{ActualFrequencySource, collect};
    use crate::SysFs;

    #[test]
    fn policies_prefer_average_and_keep_scaling_frequency_separate() {
        let directory = tempdir().expect("create CPUFreq root");
        let policy = directory.path().join("devices/system/cpu/cpufreq/policy2");
        fs::create_dir_all(&policy).expect("create policy");
        fs::create_dir_all(directory.path().join("devices/system/cpu")).expect("create CPU root");
        fs::write(directory.path().join("devices/system/cpu/online"), "0-3\n")
            .expect("write online CPUs");
        for (name, value) in [
            ("related_cpus", "0-3\n"),
            ("affected_cpus", "0 2\n"),
            ("scaling_driver", "intel_pstate\n"),
            ("cpuinfo_avg_freq", "2450000\n"),
            ("cpuinfo_cur_freq", "2300000\n"),
            ("cpuinfo_min_freq", "800000\n"),
            ("cpuinfo_max_freq", "3600000\n"),
            ("scaling_cur_freq", "2200000\n"),
            ("scaling_min_freq", "1000000\n"),
            ("scaling_max_freq", "3200000\n"),
        ] {
            fs::write(policy.join(name), value).expect("write policy attribute");
        }

        let observed =
            collect(&SysFs::new(directory.path().to_path_buf()), true, true).expect("collect");
        assert_eq!(observed.policies.len(), 1);
        assert_eq!(observed.policies[0].policy_id, 2);
        assert_eq!(
            observed.policies[0].actual_source,
            ActualFrequencySource::CpuinfoAverage
        );
        assert_eq!(observed.policies[0].related_cpus.as_deref(), Some("0-3"));
        assert_eq!(observed.samples[0].actual_frequency_hz, Some(2_450_000_000));
        assert_eq!(observed.samples[0].scaling_cur_freq_hz, Some(2_200_000_000));
        assert_eq!(observed.samples[0].online_cpus, Some(2));
    }

    #[test]
    fn a_malformed_average_falls_through_to_current() {
        let directory = tempdir().expect("create CPUFreq root");
        let policy = directory.path().join("devices/system/cpu/cpufreq/policy0");
        fs::create_dir_all(&policy).expect("create policy");
        fs::write(policy.join("cpuinfo_avg_freq"), "not-a-frequency\n")
            .expect("write malformed average");
        fs::write(policy.join("cpuinfo_cur_freq"), "2300000\n").expect("write current");

        let observed =
            collect(&SysFs::new(directory.path().to_path_buf()), true, true).expect("collect");
        assert_eq!(
            observed.policies[0].actual_source,
            ActualFrequencySource::CpuinfoCurrent
        );
        assert_eq!(observed.samples[0].actual_frequency_hz, Some(2_300_000_000));
    }

    #[test]
    fn each_observation_uses_the_first_source_it_can_parse() {
        let directory = tempdir().expect("create CPUFreq root");
        let policy = directory.path().join("devices/system/cpu/cpufreq/policy0");
        fs::create_dir_all(&policy).expect("create policy");
        fs::write(policy.join("cpuinfo_avg_freq"), "2400000\n").expect("write average");
        fs::write(policy.join("cpuinfo_cur_freq"), "2300000\n").expect("write current");
        let sys = SysFs::new(directory.path().to_path_buf());
        let first = collect(&sys, true, true).expect("collect first sample");
        assert_eq!(first.samples[0].actual_frequency_hz, Some(2_400_000_000));

        fs::remove_file(policy.join("cpuinfo_avg_freq")).expect("remove chosen average");
        let second = collect(&sys, true, true).expect("collect second sample");
        assert_eq!(
            second.policies[0].actual_source,
            ActualFrequencySource::CpuinfoCurrent
        );
        assert_eq!(second.samples[0].actual_frequency_hz, Some(2_300_000_000));
    }
}
