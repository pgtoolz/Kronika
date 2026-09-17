//! Types `1_201_001` and `1_201_002`: cgroup CPU counters and limits.

use crate::{Section, StrId, Ts};

/// CPU usage and throttling for one cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_201_001,
    name = "os_cgroup_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupCpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: i64,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: i64,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: i64,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: i64,
    /// Number of CPU throttling events.
    #[column(c, unit = count)]
    pub nr_throttled: i64,
    /// CPU quota per period, microseconds (`-1` means unlimited).
    #[column(g, unit = microseconds)]
    pub quota_usec: i64,
    /// CPU quota period.
    #[column(g, unit = microseconds)]
    pub period_usec: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Selected-ancestor CPU counters with unavailable controller fields kept null.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_201_003,
    name = "os_cgroup_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupCpuV3 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Recorded selected directory identity for counter continuity.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: i64,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: i64,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: i64,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: Option<i64>,
    /// Number of CPU throttling events.
    #[column(c, unit = count)]
    pub nr_throttled: Option<i64>,
    /// CPU quota per period, microseconds (`-1` means unlimited).
    #[column(g, unit = microseconds)]
    pub quota_usec: Option<i64>,
    /// CPU quota period.
    #[column(g, unit = microseconds)]
    pub period_usec: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Type `1_201_002`, retained so existing WAL and ZMS with `cpuset_cpus` stay readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_201_002,
    name = "os_cgroup_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupCpuV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: i64,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: i64,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: i64,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: i64,
    /// Number of CPU throttling events.
    #[column(c, unit = count)]
    pub nr_throttled: i64,
    /// CPU quota per period, microseconds (`-1` means unlimited).
    #[column(g, unit = microseconds)]
    pub quota_usec: i64,
    /// CPU quota period.
    #[column(g, unit = microseconds)]
    pub period_usec: i64,
    /// CPUs in the effective cpuset; `None` where the controller did not expose it.
    #[column(g, unit = count)]
    pub cpuset_cpus: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_cpu.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_cpu_ancestor.rs"]
mod ancestor_tests;
