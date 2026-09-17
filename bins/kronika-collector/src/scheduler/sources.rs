//! Scheduled groups and their configured intervals.

/// Statement and plan extensions must have at least five minutes between reads.
pub(crate) const MIN_PG_STATEMENTS_INTERVAL_SECS: u64 = 300;

/// One independently paced source group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::enum_variant_names,
    reason = "the names match the public OS section groups and their configuration"
)]
pub(crate) enum SourceKind {
    /// CPU, memory, pressure, disk, network and kernel counters; `CPUFreq` samples.
    OsCore,
    /// Mounts, CPU/block topology and `CPUFreq` policies, also refreshed at segment open.
    OsMountTopo,
    /// Process resource counters and the user names they reference.
    OsProcesses,
    /// Per-process status fields collected less often than resource counters.
    OsProcessStatus,
    /// Cgroup discovery and resource counters.
    OsCgroup,
    /// Process-to-cgroup membership.
    OsCgroupMapping,
    /// `PostgreSQL` and `PgBouncer` log files.
    Logs,
    /// Activity, locks and vacuum progress, accelerated while blocked sessions exist.
    PgActivity,
    /// Settings and server-wide counters.
    PgInstance,
    /// Table and index statistics, read separately in each discovered database.
    PgTablesAndIndexes,
    /// `pg_stat_statements`, `pg_store_plans` and their information rows.
    PgStatementsAndPlans,
}

/// Stable traversal order for schedule planning.
pub(super) const ALL_SOURCES: [SourceKind; 11] = [
    SourceKind::OsCore,
    SourceKind::OsMountTopo,
    SourceKind::OsProcesses,
    SourceKind::OsProcessStatus,
    SourceKind::OsCgroup,
    SourceKind::OsCgroupMapping,
    SourceKind::Logs,
    SourceKind::PgActivity,
    SourceKind::PgInstance,
    SourceKind::PgTablesAndIndexes,
    SourceKind::PgStatementsAndPlans,
];

/// Per-source intervals, in seconds.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_field_names,
    reason = "the names match the documented OS interval configuration"
)]
pub(crate) struct Intervals {
    pub os_core: u64,
    pub os_mount_topo: u64,
    pub os_processes: u64,
    pub os_process_status: u64,
    pub os_cgroup: u64,
    pub os_cgroup_mapping: u64,
    pub logs: u64,
    /// `KRONIKA_PG_ACTIVITY_INTERVAL_S`: ordinary activity/locks/vacuum cadence.
    pub pg_activity: u64,
    /// `KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S`: faster checks while lock waits exist.
    pub pg_activity_blocked: u64,
    /// `KRONIKA_PG_INTERVAL_S`: settings and server-wide counters.
    pub pg_instance: u64,
    /// `KRONIKA_PG_RELATIONS_INTERVAL_S`: both tables and indexes.
    pub pg_tables_and_indexes: u64,
    /// `KRONIKA_PG_STATEMENTS_INTERVAL_S`: at least five minutes, even when forced.
    pub pg_statements_and_plans: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            os_core: 10,
            os_mount_topo: 60,
            os_processes: 5,
            os_process_status: 30,
            os_cgroup: 30,
            os_cgroup_mapping: 30,
            logs: 10,
            pg_activity: 10,
            pg_activity_blocked: 5,
            pg_instance: 30,
            pg_tables_and_indexes: 300,
            pg_statements_and_plans: MIN_PG_STATEMENTS_INTERVAL_SECS,
        }
    }
}

impl Intervals {
    pub(super) const fn of(&self, kind: SourceKind) -> u64 {
        match kind {
            SourceKind::OsCore => self.os_core,
            SourceKind::OsMountTopo => self.os_mount_topo,
            SourceKind::OsProcesses => self.os_processes,
            SourceKind::OsProcessStatus => self.os_process_status,
            SourceKind::OsCgroup => self.os_cgroup,
            SourceKind::OsCgroupMapping => self.os_cgroup_mapping,
            SourceKind::Logs => self.logs,
            SourceKind::PgActivity => self.pg_activity,
            SourceKind::PgInstance => self.pg_instance,
            SourceKind::PgTablesAndIndexes => self.pg_tables_and_indexes,
            SourceKind::PgStatementsAndPlans => self.pg_statements_and_plans,
        }
    }
}
