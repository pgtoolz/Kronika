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
mod tests {
    use super::OsCgroupMapping;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsCgroupMapping::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsCgroupMapping::CONTRACT;
        assert_eq!(c.type_id.get(), 1_200_001);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[OsCgroupMapping {
            ts: Ts(1),
            pid: 10,
            starttime: Ts(100),
            cgroup_path: StrId(11),
            scope: 1,
        }]);
    }
}
