//! Event source names and projected field contracts.

use super::EventSource;

impl EventSource {
    pub(super) const GROUPS: [Self; 7] = [
        Self::Errors,
        Self::Checkpoints,
        Self::Autovacuum,
        Self::SlowQueries,
        Self::LockWaits,
        Self::Lifecycle,
        Self::Pgbouncer,
    ];
    pub(super) const OCCURRENCES: [Self; 8] = [
        Self::Errors,
        Self::Checkpoints,
        Self::Autovacuum,
        Self::SlowQueries,
        Self::LockWaits,
        Self::TempFiles,
        Self::Lifecycle,
        Self::Pgbouncer,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Errors => "pg_log_errors",
            Self::Checkpoints => "pg_log_checkpoints",
            Self::Autovacuum => "pg_log_autovacuum",
            Self::SlowQueries => "pg_log_slow_queries",
            Self::LockWaits => "pg_log_lock_waits",
            Self::TempFiles => "pg_log_temp_files",
            Self::Lifecycle => "pg_log_lifecycle",
            Self::Pgbouncer => "pgbouncer_events",
        }
    }

    pub(super) fn parse(name: &str) -> Option<Self> {
        Self::OCCURRENCES
            .into_iter()
            .find(|source| source.as_str() == name)
    }

    pub(super) const fn group_fields(self) -> &'static [&'static str] {
        match self {
            Self::Errors => &[
                "severity", "category", "sqlstate", "pattern", "count", "database", "username",
            ],
            Self::Checkpoints => &[
                "phase",
                "reason",
                "seconds_apart",
                "buffers_written",
                "sync_ms",
            ],
            Self::Autovacuum => &[
                "kind",
                "relation",
                "tuples_removed",
                "tuples_dead_not_removable",
                "elapsed_ms",
            ],
            Self::SlowQueries => &["pattern", "count", "max_duration_ms", "total_duration_ms"],
            Self::LockWaits => &["kind", "pid", "lock_target", "duration_ms", "holding_pids"],
            Self::Lifecycle => &["kind", "pid", "signal", "shutdown_mode"],
            Self::Pgbouncer => &[
                "source_file",
                "level",
                "database",
                "username",
                "host",
                "text",
                "pid",
                "side",
                "port",
                "age_s",
            ],
            Self::TempFiles => &[],
        }
    }

    pub(super) const fn occurrence_fields(self) -> &'static [&'static str] {
        match self {
            Self::Errors => &[
                "system_identifier",
                "source_file",
                "severity",
                "category",
                "sqlstate",
                "pattern",
                "count",
                "database",
                "username",
            ],
            Self::Checkpoints => &[
                "system_identifier",
                "source_file",
                "phase",
                "seconds_apart",
                "buffers_written",
                "write_ms",
                "sync_ms",
                "total_ms",
                "distance_kb",
                "estimate_kb",
                "wal_added",
                "wal_removed",
                "wal_recycled",
                "sync_files",
                "longest_sync_ms",
                "average_sync_ms",
            ],
            Self::Autovacuum => &[
                "system_identifier",
                "source_file",
                "kind",
                "relation",
                "index_scans",
                "pages_removed",
                "pages_remaining",
                "tuples_removed",
                "tuples_remaining",
                "tuples_dead_not_removable",
                "elapsed_ms",
                "buffer_hits",
                "buffer_misses",
                "buffer_dirtied",
                "avg_read_rate_mbs",
                "avg_write_rate_mbs",
                "cpu_user_ms",
                "cpu_system_ms",
                "wal_records",
                "wal_fpi",
                "wal_bytes",
            ],
            Self::SlowQueries => &[
                "system_identifier",
                "source_file",
                "pattern",
                "count",
                "max_duration_ms",
                "total_duration_ms",
            ],
            Self::LockWaits => &[
                "system_identifier",
                "source_file",
                "kind",
                "pid",
                "lock_mode",
                "lock_target",
                "duration_ms",
                "holding_pids",
                "wait_queue",
            ],
            Self::TempFiles => &["system_identifier", "source_file", "path", "size_bytes"],
            Self::Lifecycle => &[
                "system_identifier",
                "source_file",
                "kind",
                "pid",
                "signal",
                "shutdown_mode",
            ],
            Self::Pgbouncer => &[
                "source_file",
                "level",
                "database",
                "username",
                "host",
                "text",
                "pid",
                "side",
                "port",
                "age_s",
            ],
        }
    }
}
