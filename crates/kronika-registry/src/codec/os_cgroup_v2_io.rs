//! Type `1_210_001`: per-device I/O counters for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// Per-device I/O counters for a discovered cgroup v2 directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_210_001,
    name = "os_cgroup_v2_io",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "major", "minor", "ts"),
    identity("cgroup_path", "cgroup_identity", "major", "minor")
)]
pub struct OsCgroupV2Io {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Block device major number.
    #[column(l)]
    pub major: u32,
    /// Block device minor number.
    #[column(l)]
    pub minor: u32,
    /// Bytes read.
    #[column(c, unit = bytes)]
    pub rbytes: Option<i64>,
    /// Bytes written.
    #[column(c, unit = bytes)]
    pub wbytes: Option<i64>,
    /// Read operations.
    #[column(c, unit = count)]
    pub rios: Option<i64>,
    /// Write operations.
    #[column(c, unit = count)]
    pub wios: Option<i64>,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_v2_io.rs"]
mod tests;
