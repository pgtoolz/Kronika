//! Type `1_205_001`: the collector's exact cgroup membership and capacity.

use crate::{Section, StrId, Ts};

/// Controller paths and effective capacity for the collector process.
///
/// `cgroup_version` is `1` for cgroup v1, `2` for cgroup v2, and `0` when the
/// version could not be determined. Optional values stay null when their
/// controller or file is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_205_001,
    name = "os_cgroup_context",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsCgroupContext {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup interface version (`0=unknown`, `1=v1`, `2=v2`).
    #[column(l)]
    pub cgroup_version: u8,
    /// Exact CPU-controller path of the collector process.
    #[column(l)]
    pub cpu_path: Option<StrId>,
    /// Exact memory-controller path of the collector process.
    #[column(l)]
    pub memory_path: Option<StrId>,
    /// Exact I/O-controller path of the collector process.
    #[column(l)]
    pub io_path: Option<StrId>,
    /// CPUs exposed by the effective cpuset file.
    #[column(g, unit = count)]
    pub cpuset_cpus: Option<i64>,
    /// Tightest validated hierarchical CPU quota; `-1` means unlimited.
    #[column(g, unit = microseconds)]
    pub effective_cpu_quota_usec: Option<i64>,
    /// Period paired with `effective_cpu_quota_usec`.
    #[column(g, unit = microseconds)]
    pub effective_cpu_period_usec: Option<i64>,
    /// Tightest validated hierarchical memory limit.
    #[column(g, unit = bytes)]
    pub effective_memory_max: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsCgroupContext;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_shape_and_nulls_roundtrip() {
        let contract = OsCgroupContext::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_205_001);
        assert_eq!(contract.sort_key, ["ts"]);
        assert!(contract.identity.is_empty());
        assert_eq!(
            contract
                .columns
                .iter()
                .map(|column| column.name)
                .collect::<Vec<_>>(),
            [
                "ts",
                "cgroup_version",
                "cpu_path",
                "memory_path",
                "io_path",
                "cpuset_cpus",
                "effective_cpu_quota_usec",
                "effective_cpu_period_usec",
                "effective_memory_max",
                "scope",
            ]
        );
        assert_eq!(lint(&[contract]), Ok(()));

        crate::assert_roundtrips(&[
            OsCgroupContext {
                ts: Ts(1),
                cgroup_version: 2,
                cpu_path: Some(StrId(10)),
                memory_path: Some(StrId(10)),
                io_path: Some(StrId(10)),
                cpuset_cpus: Some(4),
                effective_cpu_quota_usec: Some(150_000),
                effective_cpu_period_usec: Some(100_000),
                effective_memory_max: Some(536_870_912),
                scope: 3,
            },
            OsCgroupContext {
                ts: Ts(2),
                cgroup_version: 0,
                cpu_path: None,
                memory_path: None,
                io_path: None,
                cpuset_cpus: None,
                effective_cpu_quota_usec: None,
                effective_cpu_period_usec: None,
                effective_memory_max: None,
                scope: 4,
            },
        ]);
    }
}
