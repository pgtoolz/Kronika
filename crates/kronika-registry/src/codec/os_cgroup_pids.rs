//! Type `1_204_001`: cgroup thread counts and limits.

use crate::{Section, StrId, Ts};

/// Thread count and local limit for one cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_204_001,
    name = "os_cgroup_pids",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupPids {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Current number of threads identified by TIDs in this cgroup and its
    /// descendants.
    #[column(g, unit = count)]
    pub current: i64,
    /// Local thread limit; `None` means unlimited. An ancestor can impose a
    /// lower limit.
    #[column(g, unit = count)]
    pub max: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_pids.rs"]
mod tests;
