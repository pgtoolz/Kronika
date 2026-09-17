//! Type `1_200_001`: process to cgroup mapping.

use crate::{Section, StrId, Ts};

/// Snapshot mapping from a PID to a normalized cgroup path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_200_001,
    name = "os_cgroup_mapping",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct OsCgroupMapping {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Process ID.
    #[column(l)]
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    #[column(l)]
    pub starttime: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_mapping.rs"]
mod tests;
