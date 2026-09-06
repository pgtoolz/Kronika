//! Types `1_202_001` and `1_202_002`: cgroup memory usage, limits, and events.

use crate::{Section, StrId, Ts};

/// Memory usage and OOM/event counters for one cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_202_001,
    name = "os_cgroup_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupMemory {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Current memory usage.
    #[column(g, unit = bytes)]
    pub current: i64,
    /// Memory limit, bytes; `None` means unlimited.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: i64,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: i64,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: i64,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: i64,
    /// `memory.events low`.
    #[column(c, unit = count)]
    pub low_events: i64,
    /// `memory.events high`.
    #[column(c, unit = count)]
    pub high_events: i64,
    /// `memory.events max` or v1 `memory.failcnt`.
    #[column(c, unit = count)]
    pub max_events: i64,
    /// `memory.events oom`.
    #[column(c, unit = count)]
    pub oom_events: i64,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Type `1_202_002`, retained so existing WAL and ZMS with `shmem` stay readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_202_002,
    name = "os_cgroup_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupMemoryV2 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Current memory usage.
    #[column(g, unit = bytes)]
    pub current: i64,
    /// Memory limit, bytes; `None` means unlimited.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: i64,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: i64,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: i64,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: i64,
    /// Shared memory counted inside `file`.
    #[column(g, unit = bytes)]
    pub shmem: i64,
    /// `memory.events low`.
    #[column(c, unit = count)]
    pub low_events: i64,
    /// `memory.events high`.
    #[column(c, unit = count)]
    pub high_events: i64,
    /// `memory.events max` or v1 `memory.failcnt`.
    #[column(c, unit = count)]
    pub max_events: i64,
    /// `memory.events oom`.
    #[column(c, unit = count)]
    pub oom_events: i64,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::{OsCgroupMemory, OsCgroupMemoryV2};
    use crate::{Section, StrId, Ts, VerifiedSection, contract::lint};

    fn row(max: Option<i64>) -> OsCgroupMemory {
        OsCgroupMemory {
            ts: Ts(1),
            cgroup_path: StrId(10),
            current: 1024,
            max,
            anon: 100,
            file: 200,
            kernel: 30,
            slab: 20,
            low_events: 1,
            high_events: 2,
            max_events: 3,
            oom_events: 4,
            oom_kill: 5,
            scope: 1,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(
            lint(&[OsCgroupMemory::CONTRACT, OsCgroupMemoryV2::CONTRACT,]),
            Ok(())
        );
    }

    #[test]
    fn contract_shape() {
        let c = OsCgroupMemory::CONTRACT;
        assert_eq!(c.type_id.get(), 1_202_001);
        assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
        assert_eq!(c.identity, ["cgroup_path"]);
    }

    #[test]
    fn v2_contract_shape() {
        let c = OsCgroupMemoryV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_202_002);
        assert_eq!(
            c.columns
                .iter()
                .map(|column| column.name)
                .collect::<Vec<_>>(),
            [
                "ts",
                "cgroup_path",
                "current",
                "max",
                "anon",
                "file",
                "kernel",
                "slab",
                "shmem",
                "low_events",
                "high_events",
                "max_events",
                "oom_events",
                "oom_kill",
                "scope",
            ]
        );
        assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
        assert_eq!(c.identity, ["cgroup_path"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(Some(2048)), row(None)]);
    }

    #[test]
    fn unlimited_limit_survives_as_null() {
        let bytes = OsCgroupMemory::encode(&[row(None)]).expect("encode");
        let decoded =
            OsCgroupMemory::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].max, None);
    }

    #[test]
    fn v2_roundtrip() {
        crate::assert_roundtrips(&[OsCgroupMemoryV2 {
            ts: Ts(1),
            cgroup_path: StrId(10),
            current: 1024,
            max: Some(2048),
            anon: 100,
            file: 200,
            kernel: 30,
            slab: 20,
            shmem: 64,
            low_events: 1,
            high_events: 2,
            max_events: 3,
            oom_events: 4,
            oom_kill: 5,
            scope: 1,
        }]);
    }
}
