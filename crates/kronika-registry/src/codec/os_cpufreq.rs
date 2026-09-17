//! Types `1_121_001` and `1_122_001`: Linux `CPUFreq` policy reference and samples.

use crate::{Section, StrId, Ts};

/// One `CPUFreq` policy's static kernel reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_121_001,
    name = "os_cpufreq_policy",
    semantics = on_change,
    sort_key("policy_id", "ts"),
    identity("policy_id")
)]
pub struct OsCpufreqPolicy {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Numeric suffix of the kernel `policyX` directory.
    #[column(l)]
    pub policy_id: i32,
    /// Exact kernel CPU-list text from `related_cpus`.
    #[column(l)]
    pub related_cpus: Option<StrId>,
    /// `CPUFreq` scaling driver.
    #[column(l)]
    pub scaling_driver: Option<StrId>,
    /// Exact chosen actual-frequency attribute name.
    #[column(l)]
    pub actual_source: Option<StrId>,
    /// Hardware minimum frequency, hertz.
    #[column(l, unit = hertz)]
    pub cpuinfo_min_freq_hz: Option<i64>,
    /// Hardware maximum frequency, hertz.
    #[column(l, unit = hertz)]
    pub cpuinfo_max_freq_hz: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// One temporal sample for a `CPUFreq` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_122_001,
    name = "os_cpufreq",
    semantics = snapshot_full,
    sort_key("policy_id", "ts"),
    identity("policy_id")
)]
pub struct OsCpufreq {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Numeric suffix of the kernel `policyX` directory.
    #[column(l)]
    pub policy_id: i32,
    /// Exact chosen actual-frequency attribute name.
    #[column(l)]
    pub actual_source: Option<StrId>,
    /// Hardware-derived policy frequency, hertz.
    #[column(g, unit = hertz)]
    pub actual_frequency_hz: Option<i64>,
    /// `CPUFreq`'s separately reported/requested current policy frequency, hertz.
    #[column(g, unit = hertz)]
    pub scaling_cur_freq_hz: Option<i64>,
    /// Current policy lower bound, hertz.
    #[column(g, unit = hertz)]
    pub scaling_min_freq_hz: Option<i64>,
    /// Current policy upper bound, hertz.
    #[column(g, unit = hertz)]
    pub scaling_max_freq_hz: Option<i64>,
    /// Online logical CPUs currently affected by this policy.
    #[column(g, unit = count)]
    pub online_cpus: Option<i32>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cpufreq.rs"]
mod tests;
