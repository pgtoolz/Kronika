//! Type `1_209_001`: PID accounting and the exact events interface read for a discovered cgroup v2 directory.

use crate::{Section, StrId, Ts};

/// PID accounting and the exact events interface read for a discovered cgroup v2 directory.
///
/// The meaning of the ordinary events file depends on the recorded mount options
/// and kernel version; it is not universally a local limiter counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_209_001,
    name = "os_cgroup_v2_pids",
    semantics = snapshot_full,
    sort_key("cgroup_path", "cgroup_identity", "events_source", "ts"),
    identity("cgroup_path", "cgroup_identity", "events_source")
)]
pub struct OsCgroupV2Pids {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Path within the exposed cgroup hierarchy.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Directory identity; recreation starts a new counter history.
    #[column(l)]
    pub cgroup_identity: StrId,
    /// Current number of tasks.
    #[column(g, unit = count)]
    pub current: Option<i64>,
    /// Finite task limit; interpret null with `max_unlimited`.
    #[column(g, unit = count)]
    pub max: Option<i64>,
    /// Whether pids.max is unlimited; null means unavailable.
    #[column(l)]
    pub max_unlimited: Option<bool>,
    /// The max counter from the recorded `events_source`.
    #[column(c, unit = count)]
    pub failure_max: Option<i64>,
    /// Events interface: 0 unavailable, 1 pids.events.local, 2 pids.events.
    #[column(l)]
    pub events_source: u8,
}

#[cfg(test)]
mod tests {
    use super::OsCgroupV2Pids;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_and_missing_values_roundtrip() {
        let contract = OsCgroupV2Pids::CONTRACT;
        assert_eq!(contract.type_id.get(), 1_209_001);
        assert_eq!(
            contract.identity,
            ["cgroup_path", "cgroup_identity", "events_source"],
        );
        assert_eq!(lint(&[contract]), Ok(()));
        assert_eq!(
            crate::contract(contract.type_id.get())
                .expect("registered type")
                .name,
            contract.name,
        );
        let rows = [
            OsCgroupV2Pids {
                ts: Ts(1),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(2),
                current: Some(0),
                max: Some(41),
                max_unlimited: Some(false),
                failure_max: Some(42),
                events_source: 1,
            },
            OsCgroupV2Pids {
                ts: Ts(2),
                cgroup_path: StrId(1),
                cgroup_identity: StrId(3),
                current: None,
                max: None,
                max_unlimited: None,
                failure_max: None,
                events_source: 0,
            },
        ];
        crate::assert_roundtrips(&rows);
        crate::assert_roundtrips(&[
            rows[1],
            OsCgroupV2Pids {
                ts: Ts(3),
                max_unlimited: Some(true),
                ..rows[1]
            },
        ]);
    }
}
