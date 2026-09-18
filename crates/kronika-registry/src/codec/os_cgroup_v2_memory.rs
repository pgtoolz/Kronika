//! Type `1_208_001`: memory accounting, limits and separate local events for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// Memory accounting, limits and separate local events for a discovered cgroup v2 directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_208_001,
    name = "os_cgroup_v2_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupV2Memory {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Current charged memory.
    #[column(g, unit = bytes)]
    pub current: Option<i64>,
    /// Finite memory limit; interpret null with `max_unlimited`.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Whether memory.max is unlimited; null means unavailable.
    #[column(l)]
    pub max_unlimited: Option<bool>,
    /// Finite reclaim threshold; interpret null with `high_unlimited`.
    #[column(g, unit = bytes)]
    pub high: Option<i64>,
    /// Whether memory.high is unlimited; null means unavailable.
    #[column(l)]
    pub high_unlimited: Option<bool>,
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
    /// memory.events low.
    #[column(c, unit = count)]
    pub low_events: Option<i64>,
    /// memory.events high.
    #[column(c, unit = count)]
    pub high_events: Option<i64>,
    /// memory.events max.
    #[column(c, unit = count)]
    pub max_events: Option<i64>,
    /// memory.events oom.
    #[column(c, unit = count)]
    pub oom_events: Option<i64>,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: Option<i64>,
    /// memory.events.local high.
    #[column(c, unit = count)]
    pub local_high_events: Option<i64>,
    /// memory.events.local max.
    #[column(c, unit = count)]
    pub local_max_events: Option<i64>,
    /// memory.events.local oom.
    #[column(c, unit = count)]
    pub local_oom_events: Option<i64>,
    /// `memory.events.local oom_kill`.
    #[column(c, unit = count)]
    pub local_oom_kill: Option<i64>,
    /// `memory.events.local oom_group_kill`.
    #[column(c, unit = count)]
    pub local_oom_group_kill: Option<i64>,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_v2_memory.rs"]
mod tests;
