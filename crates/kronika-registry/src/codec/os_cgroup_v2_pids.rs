//! Type `1_209_001`: PID accounting and the exact events interface read for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// PID accounting and the exact events interface read for a discovered cgroup v2 directory.
///
/// The meaning of the ordinary events file depends on the recorded mount options
/// and kernel version; it is not universally a local limiter counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_209_001,
    name = "os_cgroup_v2_pids",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "events_source", "ts"),
    identity("cgroup_path", "cgroup_identity", "events_source")
)]
pub struct OsCgroupV2Pids {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Current number of tasks.
    #[column(g, unit = count)]
    pub current: Option<i64>,
    /// Finite task limit; interpret null with `max_unlimited`.
    #[column(g, unit = count)]
    pub max: Option<i64>,
    /// Whether pids.max is unlimited; null means unavailable.
    #[column(l)]
    pub max_unlimited: Option<bool>,
    /// The max counter from the recorded `events_source`.
    #[column(c, unit = count)]
    pub failure_max: Option<i64>,
    /// Events interface: 0 unavailable, 1 pids.events.local, 2 pids.events.
    #[column(l)]
    pub events_source: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_v2_pids.rs"]
mod tests;
