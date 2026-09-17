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
    /// ``PostgreSQL`` and ``PgBouncer`` log files.
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
#[derive(Debug, Clone, Copy, clap::Args)]
#[allow(
    clippy::struct_field_names,
    reason = "the names match the documented OS interval configuration"
)]
pub(crate) struct Intervals {
    /// CPU, memory, disks, network, and pressure.
    #[arg(long = "os-core-interval-s", env = "KRONIKA_OS_CORE_INTERVAL_S", default_value_t = Self::default().os_core, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_core: u64,
    /// Mounts, capacity, and device topology.
    #[arg(long = "os-mount-topo-interval-s", env = "KRONIKA_OS_MOUNTTOPO_INTERVAL_S", default_value_t = Self::default().os_mount_topo, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_mount_topo: u64,
    /// Process counters.
    #[arg(long = "os-process-interval-s", env = "KRONIKA_OS_PROCESS_INTERVAL_S", default_value_t = Self::default().os_processes, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_processes: u64,
    /// Process status details.
    #[arg(long = "os-process-status-interval-s", env = "KRONIKA_OS_PROCESS_STATUS_INTERVAL_S", default_value_t = Self::default().os_process_status, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_process_status: u64,
    /// Accessible cgroup v2 groups.
    #[arg(long = "os-cgroup-interval-s", env = "KRONIKA_OS_CGROUP_INTERVAL_S", default_value_t = Self::default().os_cgroup, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_cgroup: u64,
    /// Process-to-cgroup mappings.
    #[arg(long = "os-cgroup-mapping-interval-s", env = "KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S", default_value_t = Self::default().os_cgroup_mapping, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub os_cgroup_mapping: u64,
    /// Configured `PostgreSQL` and `PgBouncer` logs.
    #[arg(long = "log-interval-s", env = "KRONIKA_LOG_INTERVAL_S", default_value_t = Self::default().logs, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub logs: u64,
    /// Activity, lock waits, and VACUUM progress.
    #[arg(long = "pg-activity-interval-s", env = "KRONIKA_PG_ACTIVITY_INTERVAL_S", default_value_t = Self::default().pg_activity, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub pg_activity: u64,
    /// Activity during lock waits; capped by the normal activity interval.
    #[arg(long = "pg-activity-blocked-interval-s", env = "KRONIKA_PG_ACTIVITY_BLOCKED_INTERVAL_S", default_value_t = Self::default().pg_activity_blocked, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub pg_activity_blocked: u64,
    /// `PostgreSQL` server counters and settings.
    #[arg(long = "pg-instance-interval-s", env = "KRONIKA_PG_INTERVAL_S", default_value_t = Self::default().pg_instance, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub pg_instance: u64,
    /// Tables and indexes in each database.
    #[arg(long = "pg-relations-interval-s", env = "KRONIKA_PG_RELATIONS_INTERVAL_S", default_value_t = Self::default().pg_tables_and_indexes, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
    pub pg_tables_and_indexes: u64,
    /// Statements, plans, and their info views; minimum 300 seconds, including SIGUSR2.
    #[arg(long = "pg-statements-interval-s", env = "KRONIKA_PG_STATEMENTS_INTERVAL_S", default_value_t = Self::default().pg_statements_and_plans, value_parser = crate::config::values::number::<u64>, help_heading = "Collection intervals (seconds)", hide_env_values = true)]
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
