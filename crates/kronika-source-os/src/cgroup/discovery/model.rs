//! Nullable readings from one discovered directory.

/// One directory, including directories without resource files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredGroup {
    /// Unix microseconds shared by this discovery pass.
    pub ts: i64,
    /// Path relative to the recorded mount root.
    pub cgroup_path: String,
    /// Object and exposed hierarchy identity.
    pub cgroup_identity: String,
    /// Exposed hierarchy root from mountinfo.
    pub mount_root: String,
    /// Observed parent object, absent at an exposed root.
    pub parent_identity: Option<String>,
    /// Filesystem device identity, used to match a primary selection.
    pub device: u64,
    /// Directory inode identity, used to match a primary selection.
    pub inode: u64,
    /// The mount requests local ordinary memory events.
    pub memory_localevents: bool,
    /// The mount requests local ordinary PID events.
    pub pids_localevents: bool,
    /// CPU counters and local configured limits.
    pub cpu: DiscoveredCpu,
    /// Memory counters and local configured limits.
    pub memory: DiscoveredMemory,
    /// PID counters and the source of failure events.
    pub pids: DiscoveredPids,
}

/// CPU values from this directory; no inherited or child substitutions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoveredCpu {
    /// Total CPU usage, microseconds.
    pub usage_usec: Option<i64>,
    /// User CPU usage, microseconds.
    pub user_usec: Option<i64>,
    /// System CPU usage, microseconds.
    pub system_usec: Option<i64>,
    /// Bandwidth periods.
    pub nr_periods: Option<i64>,
    /// Throttled periods.
    pub nr_throttled: Option<i64>,
    /// Throttled time, microseconds.
    pub throttled_usec: Option<i64>,
    /// CPU quota, microseconds; -1 is a recorded `max`.
    pub quota_usec: Option<i64>,
    /// CPU quota period, microseconds.
    pub period_usec: Option<i64>,
    /// Number of CPUs in the effective cpuset.
    pub cpuset_cpus: Option<i64>,
}

/// Memory values from this directory, in bytes or cumulative event counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveredMemory {
    /// Current charged bytes.
    pub current: Option<i64>,
    /// Maximum bytes; -1 is a recorded `max`.
    pub max: Option<i64>,
    /// High boundary in bytes; -1 is a recorded `max`.
    pub high: Option<i64>,
    /// Anonymous bytes.
    pub anon: Option<i64>,
    /// File-backed bytes.
    pub file: Option<i64>,
    /// Kernel bytes.
    pub kernel: Option<i64>,
    /// Slab bytes.
    pub slab: Option<i64>,
    /// Ordinary low events.
    pub low_events: Option<i64>,
    /// Ordinary high events.
    pub high_events: Option<i64>,
    /// Ordinary maximum events.
    pub max_events: Option<i64>,
    /// Ordinary OOM events.
    pub oom_events: Option<i64>,
    /// Ordinary OOM kills.
    pub oom_kill: Option<i64>,
    /// Local high events.
    pub local_high_events: Option<i64>,
    /// Local maximum events.
    pub local_max_events: Option<i64>,
    /// Local OOM events.
    pub local_oom_events: Option<i64>,
    /// Local OOM kills.
    pub local_oom_kill: Option<i64>,
    /// Local OOM group kills.
    pub local_oom_group_kill: Option<i64>,
}

/// PID controller readings without a kernel-version inference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiscoveredPids {
    /// Current threads in this group and descendants.
    pub current: Option<i64>,
    /// Configured maximum; -1 is a recorded `max`.
    pub max: Option<i64>,
    /// The selected events file's `max` count.
    pub failure_max: Option<i64>,
    /// 0 unavailable, 1 `pids.events.local`, 2 `pids.events`.
    pub events_source: u8,
}

/// One device row, emitted without materializing the directory's entire I/O list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredIo {
    /// Unix microseconds shared by this discovery pass.
    pub ts: i64,
    /// Path relative to the recorded mount root.
    pub cgroup_path: String,
    /// Directory identity matching the inventory row.
    pub cgroup_identity: String,
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

/// Borrowed discovery output; callbacks may append it immediately to bounded buffers.
#[derive(Debug)]
pub enum DiscoveryRow<'a> {
    /// Directory inventory and independently nullable resource fields.
    Group(&'a DiscoveredGroup),
    /// One group's device counters.
    Io(&'a DiscoveredIo),
}

/// Coverage and best-effort acquisition errors from one completed walk.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoveryStats {
    /// Unique directories emitted.
    pub groups: usize,
    /// Device rows emitted.
    pub io_rows: usize,
    /// Metric files successfully opened, including empty files.
    pub metric_files_read: usize,
    /// Directories that disappeared or could not be accessed.
    pub skipped_directories: usize,
    /// Present metric files that could not be read or exceeded the per-file bound.
    pub metric_errors: usize,
    /// First acquisition error, with its directory and file when available.
    pub first_error: Option<String>,
}
