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
mod tests {
    use super::OsCgroupV2Io;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_and_missing_values_roundtrip() {
        let contract = OsCgroupV2Io::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_210_001);
        assert_eq!(
            contract.identity,
            ["cgroup_path", "cgroup_identity", "major", "minor"]
        );
        assert_eq!(lint(&[contract]), Ok(()));
        assert_eq!(
            crate::contract(contract.type_id.get())
                .expect("registered type")
                .name,
            contract.name,
        );
        crate::assert_roundtrips(&[
            OsCgroupV2Io {
                ts: Ts(1),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(2),
                major: 8,
                minor: 0,
                rbytes: Some(0),
                wbytes: Some(41),
                rios: Some(42),
                wios: Some(43),
            },
            OsCgroupV2Io {
                ts: Ts(2),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(3),
                major: 8,
                minor: 0,
                rbytes: None,
                wbytes: None,
                rios: None,
                wios: None,
            },
        ]);
    }
}
