//! Type `1_207_001`: CPU accounting and limits for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// CPU accounting and limits for a discovered cgroup v2 directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_207_001,
    name = "os_cgroup_v2_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupV2Cpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: Option<i64>,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: Option<i64>,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: Option<i64>,
    /// Elapsed CPU enforcement periods.
    #[column(c, unit = count)]
    pub nr_periods: Option<i64>,
    /// Periods in which this group was throttled.
    #[column(c, unit = count)]
    pub nr_throttled: Option<i64>,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: Option<i64>,
    /// CPU quota per period; -1 means recorded unlimited.
    #[column(g, unit = microseconds)]
    pub quota_usec: Option<i64>,
    /// Period paired with `quota_usec`.
    #[column(g, unit = microseconds)]
    pub period_usec: Option<i64>,
    /// CPU count from cpuset.cpus.effective.
    #[column(g, unit = count)]
    pub cpuset_cpus: Option<i64>,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_v2_cpu.rs"]
mod tests;
