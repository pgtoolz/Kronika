//! Type `1_206_001`: discovered cgroup v2 directories, including groups without resource files.

use crate::{Section, StrId, Ts};

/// Discovered cgroup v2 directories, including groups without resource files.
///
/// A visible mount root does not establish a host, pod or `PostgreSQL` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_206_001,
    name = "os_cgroup_v2_group",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupV2Group {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Root exposed by the chosen cgroup2 mount.
    #[column(l)]
    pub mount_root: StrId,
    /// Readable parent identity within the exposed hierarchy, when known.
    #[column(l)]
    pub parent_identity: Option<StrId>,
    /// The chosen mount has the `memory_localevents` option.
    #[column(l)]
    pub memory_localevents: bool,
    /// The chosen mount has the `pids_localevents` option.
    #[column(l)]
    pub pids_localevents: bool,
}

#[cfg(test)]
#[path = "../tests/codec/os_cgroup_v2_group.rs"]
mod tests;
