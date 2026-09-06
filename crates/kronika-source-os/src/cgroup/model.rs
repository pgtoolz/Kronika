//! Cgroup rows before string interning.

/// Cgroup collection output before string interning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CgroupCollection {
    /// CPU rows.
    pub cpu: Vec<CgroupCpuRow>,
    /// Selected-ancestor CPU rows with nullable unavailable fields.
    pub ancestor_cpu: Vec<AncestorCpuRow>,
    /// Memory rows.
    pub memory: Vec<CgroupMemoryRow>,
    /// Selected-ancestor memory with nullable unsupported fields.
    pub ancestor_memory: Vec<AncestorMemoryRow>,
    /// I/O rows.
    pub io: Vec<CgroupIoRow>,
    /// PIDs rows.
    pub pids: Vec<CgroupPidsRow>,
    /// The I/O section was omitted because its hard row ceiling was exceeded.
    pub io_omitted: bool,
}

/// The collector process's own cgroup membership at one timestamp.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CgroupContextRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Cgroup interface version (`0=unknown`, `1=v1`, `2=v2`).
    pub cgroup_version: u8,
    /// Exact CPU-controller path.
    pub cpu_path: Option<String>,
    /// Exact memory-controller path.
    pub memory_path: Option<String>,
    /// Exact I/O-controller path.
    pub io_path: Option<String>,
    /// CPU count from the effective cpuset file.
    pub cpuset_cpus: Option<i64>,
    /// Tightest validated CPU quota in microseconds; `-1` means unlimited.
    pub effective_cpu_quota_usec: Option<i64>,
    /// Period paired with the effective CPU quota, in microseconds.
    pub effective_cpu_period_usec: Option<i64>,
    /// Tightest validated memory limit, bytes; absent when unlimited or unknown.
    pub effective_memory_max: Option<i64>,
}

/// CPU metrics for one cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupCpuRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Normalized cgroup path.
    pub cgroup_path: String,
    /// Total usage, microseconds.
    pub usage_usec: i64,
    /// User usage, microseconds.
    pub user_usec: i64,
    /// System usage, microseconds.
    pub system_usec: i64,
    /// Throttled time, microseconds.
    pub throttled_usec: i64,
    /// Number of throttling events.
    pub nr_throttled: i64,
    /// Quota, microseconds; `-1` means unlimited.
    pub quota_usec: i64,
    /// Period, microseconds.
    pub period_usec: i64,
}

/// Memory metrics for one cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupMemoryRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Normalized cgroup path.
    pub cgroup_path: String,
    /// Current usage, bytes.
    pub current: i64,
    /// Limit, bytes; `None` means unlimited.
    pub max: Option<i64>,
    /// Anonymous memory, bytes.
    pub anon: i64,
    /// File-backed memory, bytes.
    pub file: i64,
    /// Kernel memory, bytes.
    pub kernel: i64,
    /// Slab memory, bytes.
    pub slab: i64,
    /// Low memory events.
    pub low_events: i64,
    /// High memory events.
    pub high_events: i64,
    /// Max boundary events.
    pub max_events: i64,
    /// OOM events.
    pub oom_events: i64,
    /// OOM kills.
    pub oom_kill: i64,
}

/// I/O metrics for one cgroup/device pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupIoRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Normalized cgroup path.
    pub cgroup_path: String,
    /// Device major number.
    pub major: u32,
    /// Device minor number.
    pub minor: u32,
    /// Bytes read.
    pub rbytes: Option<i64>,
    /// Bytes written.
    pub wbytes: Option<i64>,
    /// Read operations.
    pub rios: Option<i64>,
    /// Write operations.
    pub wios: Option<i64>,
}

/// PID controller metrics for one cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupPidsRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Normalized cgroup path.
    pub cgroup_path: String,
    /// Current thread count identified by TIDs in this cgroup and its descendants.
    pub current: i64,
    /// Local thread limit; `None` means unlimited.
    pub max: Option<i64>,
}

/// Selected-ancestor memory counters before string interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestorMemoryRow {
    /// Collection timestamp.
    pub ts: i64,
    /// Selected hierarchy path.
    pub cgroup_path: String,
    /// Recorded current usage, bytes.
    pub current: i64,
    /// Recorded finite limit, bytes.
    pub max: Option<i64>,
    /// Validated unlimited marker; absent means unknown limit.
    pub max_unlimited: Option<bool>,
    /// Anonymous bytes.
    pub anon: Option<i64>,
    /// File-backed bytes.
    pub file: Option<i64>,
    /// Kernel bytes.
    pub kernel: Option<i64>,
    /// Slab bytes.
    pub slab: Option<i64>,
    /// Low events.
    pub low_events: Option<i64>,
    /// High events.
    pub high_events: Option<i64>,
    /// Maximum-boundary events.
    pub max_events: Option<i64>,
    /// OOM events.
    pub oom_events: Option<i64>,
    /// OOM kills.
    pub oom_kill: Option<i64>,
}

/// Selected CPU counters before string interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AncestorCpuRow {
    /// Collection timestamp.
    pub ts: i64,
    /// Selected controller path.
    pub cgroup_path: String,
    /// Total CPU time, microseconds.
    pub usage_usec: i64,
    /// User CPU time, microseconds.
    pub user_usec: i64,
    /// System CPU time, microseconds.
    pub system_usec: i64,
    /// Throttled time, microseconds.
    pub throttled_usec: Option<i64>,
    /// Throttling event count.
    pub nr_throttled: Option<i64>,
    /// Validated quota; -1 means recorded unlimited.
    pub quota_usec: Option<i64>,
    /// Period paired with quota.
    pub period_usec: Option<i64>,
}
