//! Types `1_202_001` and `1_202_002`: cgroup memory usage, limits, and events.

use crate::{Section, StrId, Ts};

/// Memory usage and OOM/event counters for one cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_202_001,
    name = "os_cgroup_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupMemory {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Current memory usage.
    #[column(g, unit = bytes)]
    pub current: i64,
    /// Memory limit, bytes; `None` means unlimited.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: i64,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: i64,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: i64,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: i64,
    /// `memory.events low`.
    #[column(c, unit = count)]
    pub low_events: i64,
    /// `memory.events high`.
    #[column(c, unit = count)]
    pub high_events: i64,
    /// `memory.events max` or v1 `memory.failcnt`.
    #[column(c, unit = count)]
    pub max_events: i64,
    /// `memory.events oom`.
    #[column(c, unit = count)]
    pub oom_events: i64,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Selected-ancestor memory; unsupported or unreadable fields remain null.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_202_003,
    name = "os_cgroup_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupMemoryV3 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Recorded selected directory identity for counter continuity.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Current memory usage.
    #[column(g, unit = bytes)]
    pub current: i64,
    /// Recorded memory limit, bytes; null is interpreted with `max_unlimited`.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: Option<i64>,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: Option<i64>,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: Option<i64>,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: Option<i64>,
    /// `memory.events low`.
    #[column(c, unit = count)]
    pub low_events: Option<i64>,
    /// `memory.events high`.
    #[column(c, unit = count)]
    pub high_events: Option<i64>,
    /// `memory.events max`.
    #[column(c, unit = count)]
    pub max_events: Option<i64>,
    /// `memory.events oom`.
    #[column(c, unit = count)]
    pub oom_events: Option<i64>,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: Option<i64>,
    /// Whether a successfully read limit is unlimited; null means unavailable.
    #[column(l)]
    pub max_unlimited: Option<bool>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Type `1_202_002`, retained so existing WAL and ZMS with `shmem` stay readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_202_002,
    name = "os_cgroup_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupMemoryV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Current memory usage.
    #[column(g, unit = bytes)]
    pub current: i64,
    /// Memory limit, bytes; `None` means unlimited.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: i64,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: i64,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: i64,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: i64,
    /// Shared memory counted inside `file`.
    #[column(g, unit = bytes)]
    pub shmem: i64,
    /// `memory.events low`.
    #[column(c, unit = count)]
    pub low_events: i64,
    /// `memory.events high`.
    #[column(c, unit = count)]
    pub high_events: i64,
    /// `memory.events max` or v1 `memory.failcnt`.
    #[column(c, unit = count)]
    pub max_events: i64,
    /// `memory.events oom`.
    #[column(c, unit = count)]
    pub oom_events: i64,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_memory.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_memory_ancestor.rs"]
mod ancestor_tests;
