//! Type `1_207_001`: CPU accounting and limits for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// CPU accounting and limits for a discovered cgroup v2 directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_207_001,
    name = "os_cgroup_v2_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupV2Cpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: Option<i64>,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: Option<i64>,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: Option<i64>,
    /// Elapsed CPU enforcement periods.
    #[column(c, unit = count)]
    pub nr_periods: Option<i64>,
    /// Periods in which this group was throttled.
    #[column(c, unit = count)]
    pub nr_throttled: Option<i64>,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: Option<i64>,
    /// CPU quota per period; -1 means recorded unlimited.
    #[column(g, unit = microseconds)]
    pub quota_usec: Option<i64>,
    /// Period paired with `quota_usec`.
    #[column(g, unit = microseconds)]
    pub period_usec: Option<i64>,
    /// CPU count from cpuset.cpus.effective.
    #[column(g, unit = count)]
    pub cpuset_cpus: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::OsCgroupV2Cpu;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_and_missing_values_roundtrip() {
        let contract = OsCgroupV2Cpu::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_207_001);
        assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
        assert_eq!(lint(&[contract]), Ok(()));
        assert_eq!(
            crate::contract(contract.type_id.get())
                .expect("registered type")
                .name,
            contract.name,
        );
        crate::assert_roundtrips(&[
            OsCgroupV2Cpu {
                ts: Ts(1),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(2),
                usage_usec: Some(41),
                user_usec: Some(42),
                system_usec: Some(43),
                nr_periods: Some(44),
                nr_throttled: Some(45),
                throttled_usec: Some(46),
                quota_usec: Some(-1),
                period_usec: Some(100_000),
                cpuset_cpus: Some(8),
            },
            OsCgroupV2Cpu {
                ts: Ts(2),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(3),
                usage_usec: None,
                user_usec: None,
                system_usec: None,
                nr_periods: None,
                nr_throttled: None,
                throttled_usec: None,
                quota_usec: None,
                period_usec: None,
                cpuset_cpus: None,
            },
        ]);
    }
}
