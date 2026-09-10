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
mod tests {
    use super::OsCgroupV2Group;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_and_missing_values_roundtrip() {
        let contract = OsCgroupV2Group::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_206_001);
        assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
        assert_eq!(lint(&[contract]), Ok(()));
        assert_eq!(
            crate::contract(contract.type_id.get())
                .expect("registered type")
                .name,
            contract.name,
        );
        crate::assert_roundtrips(&[
            OsCgroupV2Group {
                ts: Ts(1),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(2),
                mount_root: StrId(4),
                parent_identity: Some(StrId(5)),
                memory_localevents: true,
                pids_localevents: true,
            },
            OsCgroupV2Group {
                ts: Ts(2),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(3),
                mount_root: StrId(4),
                parent_identity: None,
                memory_localevents: false,
                pids_localevents: false,
            },
        ]);
    }
}
