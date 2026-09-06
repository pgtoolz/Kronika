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
mod tests {
    use super::OsCgroupIo;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsCgroupIo::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsCgroupIo::CONTRACT;
        assert_eq!(c.type_id.get(), 1_203_002);
        assert_eq!(c.sort_key, ["cgroup_path", "major", "minor", "ts"]);
        assert_eq!(c.identity, ["cgroup_path", "major", "minor"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[OsCgroupIo {
            ts: Ts(1),
            cgroup_path: StrId(10),
            major: 8,
            minor: 0,
            rbytes: Some(100),
            wbytes: Some(200),
            rios: Some(3),
            wios: Some(4),
            scope: 1,
        }]);
    }
}

#[cfg(test)]
mod ancestor_tests {
    use super::OsCgroupIoV2;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn ancestor_io_identity_and_unknown_counters_roundtrip() {
        let contract = OsCgroupIoV2::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_203_003);
        assert_eq!(
            contract.identity,
            ["cgroup_path", "cgroup_identity", "major", "minor"]
        );
        assert_eq!(lint(&[contract]), Ok(()));
        let row = OsCgroupIoV2 {
            ts: Ts(1),
            cgroup_path: StrId(1),
            cgroup_identity: StrId(2),
            major: 8,
            minor: 0,
            rbytes: Some(100),
            wbytes: None,
            rios: None,
            wios: None,
            scope: 4,
        };
        crate::assert_roundtrips(&[
            row,
            OsCgroupIoV2 {
                ts: Ts(2),
                cgroup_identity: StrId(3),
                rbytes: Some(900),
                ..row
            },
        ]);
    }
}
