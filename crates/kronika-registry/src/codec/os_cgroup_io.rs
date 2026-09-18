//! Type `1_203_002`: per-device cgroup I/O counters.

use crate::{Section, StrId, Ts};

/// Per-device cgroup I/O counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_203_002,
    name = "os_cgroup_io",
    semantics = snapshot_full,
    sort_key("cgroup_path", "major", "minor", "ts"),
    identity("cgroup_path", "major", "minor")
)]
pub struct OsCgroupIo {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
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
    /// Read I/O operations.
    #[column(c, unit = count)]
    pub rios: Option<i64>,
    /// Write I/O operations.
    #[column(c, unit = count)]
    pub wios: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Selected-ancestor per-device I/O counters with recorded directory identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_203_003,
    name = "os_cgroup_io",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "major", "minor", "ts"),
    identity("cgroup_path", "cgroup_identity", "major", "minor")
)]
pub struct OsCgroupIoV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Recorded selected directory identity for counter continuity.
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
    /// Read I/O operations.
    #[column(c, unit = count)]
    pub rios: Option<i64>,
    /// Write I/O operations.
    #[column(c, unit = count)]
    pub wios: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_io.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_io_ancestor.rs"]
mod ancestor_tests;
