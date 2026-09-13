//! Type `1_208_001`: memory accounting, limits and separate local events for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// Memory accounting, limits and separate local events for a discovered cgroup v2 directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_208_001,
    name = "os_cgroup_v2_memory",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "ts"),
    identity("cgroup_path", "cgroup_identity")
)]
pub struct OsCgroupV2Memory {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Current charged memory.
    #[column(g, unit = bytes)]
    pub current: Option<i64>,
    /// Finite memory limit; interpret null with `max_unlimited`.
    #[column(g, unit = bytes)]
    pub max: Option<i64>,
    /// Whether memory.max is unlimited; null means unavailable.
    #[column(l)]
    pub max_unlimited: Option<bool>,
    /// Finite reclaim threshold; interpret null with `high_unlimited`.
    #[column(g, unit = bytes)]
    pub high: Option<i64>,
    /// Whether memory.high is unlimited; null means unavailable.
    #[column(l)]
    pub high_unlimited: Option<bool>,
    /// Anonymous memory.
    #[column(g, unit = bytes)]
    pub anon: Option<i64>,
    /// File-backed memory.
    #[column(g, unit = bytes)]
    pub file: Option<i64>,
    /// Kernel memory.
    #[column(g, unit = bytes)]
    pub kernel: Option<i64>,
    /// Slab memory.
    #[column(g, unit = bytes)]
    pub slab: Option<i64>,
    /// memory.events low.
    #[column(c, unit = count)]
    pub low_events: Option<i64>,
    /// memory.events high.
    #[column(c, unit = count)]
    pub high_events: Option<i64>,
    /// memory.events max.
    #[column(c, unit = count)]
    pub max_events: Option<i64>,
    /// memory.events oom.
    #[column(c, unit = count)]
    pub oom_events: Option<i64>,
    /// `memory.events oom_kill`.
    #[column(c, unit = count)]
    pub oom_kill: Option<i64>,
    /// memory.events.local high.
    #[column(c, unit = count)]
    pub local_high_events: Option<i64>,
    /// memory.events.local max.
    #[column(c, unit = count)]
    pub local_max_events: Option<i64>,
    /// memory.events.local oom.
    #[column(c, unit = count)]
    pub local_oom_events: Option<i64>,
    /// `memory.events.local oom_kill`.
    #[column(c, unit = count)]
    pub local_oom_kill: Option<i64>,
    /// `memory.events.local oom_group_kill`.
    #[column(c, unit = count)]
    pub local_oom_group_kill: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::OsCgroupV2Memory;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_and_missing_values_roundtrip() {
        let contract = OsCgroupV2Memory::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_208_001);
        assert_eq!(contract.identity, ["cgroup_path", "cgroup_identity"]);
        assert_eq!(lint(&[contract]), Ok(()));
        assert_eq!(
            crate::contract(contract.type_id.get())
                .expect("registered type")
                .name,
            contract.name,
        );
        let rows = [
            OsCgroupV2Memory {
                ts: Ts(1),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(2),
                current: Some(0),
                max: Some(41),
                max_unlimited: Some(false),
                high: Some(42),
                high_unlimited: Some(false),
                anon: Some(43),
                file: Some(44),
                kernel: Some(45),
                slab: Some(46),
                low_events: Some(47),
                high_events: Some(48),
                max_events: Some(49),
                oom_events: Some(50),
                oom_kill: Some(51),
                local_high_events: Some(52),
                local_max_events: Some(53),
                local_oom_events: Some(54),
                local_oom_kill: Some(55),
                local_oom_group_kill: Some(56),
            },
            OsCgroupV2Memory {
                ts: Ts(2),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(3),
                current: None,
                max: None,
                max_unlimited: None,
                high: None,
                high_unlimited: None,
                anon: None,
                file: None,
                kernel: None,
                slab: None,
                low_events: None,
                high_events: None,
                max_events: None,
                oom_events: None,
                oom_kill: None,
                local_high_events: None,
                local_max_events: None,
                local_oom_events: None,
                local_oom_kill: None,
                local_oom_group_kill: None,
            },
        ];
        crate::assert_roundtrips(&rows);
        crate::assert_roundtrips(&[
            rows[1],
            OsCgroupV2Memory {
                ts: Ts(3),
                max_unlimited: Some(true),
                high_unlimited: Some(true),
                ..rows[1]
            },
        ]);
    }
}
