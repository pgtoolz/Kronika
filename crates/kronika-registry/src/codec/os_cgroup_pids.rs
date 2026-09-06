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
mod tests {
    use super::OsCgroupPids;
    use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

    fn row(max: Option<i64>) -> OsCgroupPids {
        OsCgroupPids {
            ts: Ts(1),
            cgroup_path: StrId(10),
            current: 12,
            max,
            scope: 1,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsCgroupPids::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsCgroupPids::CONTRACT;
        assert_eq!(c.type_id.get(), 1_204_001);
        assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
        assert_eq!(c.identity, ["cgroup_path"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(Some(100)), row(None)]);
    }

    #[test]
    fn unlimited_limit_survives_as_null() {
        let bytes = OsCgroupPids::encode(&[row(None)]).expect("encode");
        let decoded =
            OsCgroupPids::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].max, None);
    }
}
