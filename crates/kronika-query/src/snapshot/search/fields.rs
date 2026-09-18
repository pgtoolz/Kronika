//! Structured search fields and derived metric dependencies.

use super::{QuantityKind, ResultField, SearchField, SearchFieldKind};

/// Structured-search field catalog for a logical snapshot section.
#[must_use]
pub fn search_fields(logical_name: &str) -> &'static [SearchField] {
    match logical_name {
        "os_process" => PROCESS_SEARCH_FIELDS,
        "os_cgroup_cpu"
        | "os_cgroup_memory"
        | "os_cgroup_io"
        | "os_cgroup_pids"
        | "os_cgroup_v2_cpu"
        | "os_cgroup_v2_memory"
        | "os_cgroup_v2_io"
        | "os_cgroup_v2_pids"
        | "os_cgroup_v2_group" => CGROUP_SEARCH_FIELDS,
        "pg_stat_statements" => STATEMENT_SEARCH_FIELDS,
        "pg_store_plans" => PLAN_SEARCH_FIELDS,
        "pg_stat_user_tables" => TABLE_SEARCH_FIELDS,
        "pg_stat_user_indexes" => INDEX_SEARCH_FIELDS,
        "pg_stat_activity" => ACTIVITY_SEARCH_FIELDS,
        "pg_locks" => LOCKS_SEARCH_FIELDS,
        "pg_stat_progress_vacuum" => VACUUM_SEARCH_FIELDS,
        "pg_stat_database" => DATABASE_SEARCH_FIELDS,
        _ => &[],
    }
}

/// Derived result definition for one public search field.
#[must_use]
pub fn result_field(logical_name: &str, key: &str) -> Option<ResultField> {
    let field = search_fields(logical_name)
        .iter()
        .find(|field| field.key == key)?;
    match field.kind {
        SearchFieldKind::Quantity(result) => Some(result),
        SearchFieldKind::Identifier { .. } | SearchFieldKind::String => None,
    }
}
const fn search_string(
    key: &'static str,
    aliases: &'static [&'static str],
    columns: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns,
        kind: SearchFieldKind::String,
    }
}

const fn search_id(
    key: &'static str,
    aliases: &'static [&'static str],
    columns: &'static [&'static str],
    signed: bool,
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns,
        kind: SearchFieldKind::Identifier { signed },
    }
}

const fn search_quantity(
    key: &'static str,
    kind: QuantityKind,
    metric: &'static str,
    dependencies: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases: &[],
        columns: &[],
        kind: SearchFieldKind::Quantity(ResultField {
            metric,
            kind,
            dependencies,
        }),
    }
}

const fn search_quantity_aliases(
    key: &'static str,
    aliases: &'static [&'static str],
    kind: QuantityKind,
    metric: &'static str,
    dependencies: &'static [&'static str],
) -> SearchField {
    SearchField {
        key,
        aliases,
        columns: &[],
        kind: SearchFieldKind::Quantity(ResultField {
            metric,
            kind,
            dependencies,
        }),
    }
}

const STATEMENT_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["query", "datname", "usename"]),
    search_id("query_id", &[], &["queryid"], true),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_quantity(
        "call_rate",
        QuantityKind::CountRate,
        "call_rate",
        &["calls"],
    ),
    search_quantity(
        "exec_time_rate",
        QuantityKind::DurationRate,
        "exec_time_rate",
        &["total_time", "total_exec_time"],
    ),
    search_quantity(
        "mean_exec",
        QuantityKind::Duration,
        "mean_exec",
        &["calls", "total_time", "total_exec_time"],
    ),
    search_quantity("row_rate", QuantityKind::CountRate, "row_rate", &["rows"]),
    search_quantity(
        "rows_per_call",
        QuantityKind::Scalar,
        "rows_per_call",
        &["calls", "rows"],
    ),
    search_quantity(
        "plan_rate",
        QuantityKind::CountRate,
        "plan_rate",
        &["plans"],
    ),
    search_quantity(
        "planning_time_rate",
        QuantityKind::DurationRate,
        "planning_time_rate",
        &["total_plan_time"],
    ),
    search_quantity(
        "planning_share",
        QuantityKind::Percentage,
        "planning_share",
        &["total_plan_time", "total_time", "total_exec_time"],
    ),
    search_quantity(
        "shared_buffer_hit_rate",
        QuantityKind::ByteRate,
        "shared_buffer_hit_rate",
        &["shared_blks_hit"],
    ),
    search_quantity(
        "shared_buffer_read_rate",
        QuantityKind::ByteRate,
        "shared_buffer_read_rate",
        &["shared_blks_read"],
    ),
    search_quantity(
        "shared_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "shared_buffer_dirty_rate",
        &["shared_blks_dirtied"],
    ),
    search_quantity(
        "shared_buffer_write_rate",
        QuantityKind::ByteRate,
        "shared_buffer_write_rate",
        &["shared_blks_written"],
    ),
    search_quantity(
        "local_buffer_hit_rate",
        QuantityKind::ByteRate,
        "local_buffer_hit_rate",
        &["local_blks_hit"],
    ),
    search_quantity(
        "local_buffer_read_rate",
        QuantityKind::ByteRate,
        "local_buffer_read_rate",
        &["local_blks_read"],
    ),
    search_quantity(
        "local_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "local_buffer_dirty_rate",
        &["local_blks_dirtied"],
    ),
    search_quantity(
        "local_buffer_write_rate",
        QuantityKind::ByteRate,
        "local_buffer_write_rate",
        &["local_blks_written"],
    ),
    search_quantity(
        "temp_buffer_read_rate",
        QuantityKind::ByteRate,
        "temp_buffer_read_rate",
        &["temp_blks_read"],
    ),
    search_quantity(
        "temp_buffer_write_rate",
        QuantityKind::ByteRate,
        "temp_buffer_write_rate",
        &["temp_blks_written"],
    ),
    search_quantity(
        "shared_read_time_rate",
        QuantityKind::DurationRate,
        "shared_read_time_rate",
        &["blk_read_time", "shared_blk_read_time"],
    ),
    search_quantity(
        "shared_write_time_rate",
        QuantityKind::DurationRate,
        "shared_write_time_rate",
        &["blk_write_time", "shared_blk_write_time"],
    ),
    search_quantity(
        "local_read_time_rate",
        QuantityKind::DurationRate,
        "local_read_time_rate",
        &["local_blk_read_time"],
    ),
    search_quantity(
        "local_write_time_rate",
        QuantityKind::DurationRate,
        "local_write_time_rate",
        &["local_blk_write_time"],
    ),
    search_quantity(
        "temp_read_time_rate",
        QuantityKind::DurationRate,
        "temp_read_time_rate",
        &["temp_blk_read_time"],
    ),
    search_quantity(
        "temp_write_time_rate",
        QuantityKind::DurationRate,
        "temp_write_time_rate",
        &["temp_blk_write_time"],
    ),
    search_quantity(
        "wal_rate",
        QuantityKind::ByteRate,
        "wal_rate",
        &["wal_bytes"],
    ),
    search_quantity(
        "wal_per_call",
        QuantityKind::Bytes,
        "wal_per_call",
        &["calls", "wal_bytes"],
    ),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit",
        &["shared_blks_hit", "shared_blks_read"],
    ),
    search_quantity(
        "buffer_per_call",
        QuantityKind::Bytes,
        "buffer_per_call",
        &[
            "calls",
            "shared_blks_hit",
            "shared_blks_read",
            "shared_blks_dirtied",
            "shared_blks_written",
            "local_blks_hit",
            "local_blks_read",
            "local_blks_dirtied",
            "local_blks_written",
            "temp_blks_read",
            "temp_blks_written",
        ],
    ),
    search_quantity(
        "exec_cv",
        QuantityKind::Scalar,
        "exec_cv",
        &[
            "mean_time",
            "stddev_time",
            "mean_exec_time",
            "stddev_exec_time",
        ],
    ),
    search_quantity(
        "min_exec_since_reset",
        QuantityKind::Duration,
        "min_exec_since_reset",
        &["min_time", "min_exec_time"],
    ),
    search_quantity(
        "max_exec_since_reset",
        QuantityKind::Duration,
        "max_exec_since_reset",
        &["max_time", "max_exec_time"],
    ),
    search_quantity(
        "mean_exec_since_reset",
        QuantityKind::Duration,
        "mean_exec_since_reset",
        &["mean_time", "mean_exec_time"],
    ),
    search_quantity(
        "stddev_exec_since_reset",
        QuantityKind::Duration,
        "stddev_exec_since_reset",
        &["stddev_time", "stddev_exec_time"],
    ),
];
const PLAN_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["plan", "datname", "usename"]),
    search_id(
        "query_id",
        &[],
        &["queryid", "queryid_stat_statements"],
        true,
    ),
    search_id("plan_id", &[], &["planid"], true),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_quantity("calls", QuantityKind::Count, "calls", &["calls"]),
    search_quantity(
        "call_rate",
        QuantityKind::CountRate,
        "call_rate",
        &["calls"],
    ),
    search_quantity(
        "exec_time_rate",
        QuantityKind::DurationRate,
        "exec_time_rate",
        &["total_time"],
    ),
    search_quantity(
        "mean_exec",
        QuantityKind::Duration,
        "mean_exec",
        &["calls", "total_time"],
    ),
    search_quantity("row_rate", QuantityKind::CountRate, "row_rate", &["rows"]),
    search_quantity(
        "rows_per_call",
        QuantityKind::Scalar,
        "rows_per_call",
        &["calls", "rows"],
    ),
    search_quantity(
        "planning_time_rate",
        QuantityKind::DurationRate,
        "planning_time_rate",
        &["total_plan_time"],
    ),
    search_quantity(
        "planning_share",
        QuantityKind::Percentage,
        "planning_share",
        &["total_plan_time", "total_time"],
    ),
    search_quantity(
        "shared_buffer_hit_rate",
        QuantityKind::ByteRate,
        "shared_buffer_hit_rate",
        &["shared_blks_hit"],
    ),
    search_quantity(
        "shared_buffer_read_rate",
        QuantityKind::ByteRate,
        "shared_buffer_read_rate",
        &["shared_blks_read"],
    ),
    search_quantity(
        "shared_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "shared_buffer_dirty_rate",
        &["shared_blks_dirtied"],
    ),
    search_quantity(
        "shared_buffer_write_rate",
        QuantityKind::ByteRate,
        "shared_buffer_write_rate",
        &["shared_blks_written"],
    ),
    search_quantity(
        "local_buffer_hit_rate",
        QuantityKind::ByteRate,
        "local_buffer_hit_rate",
        &["local_blks_hit"],
    ),
    search_quantity(
        "local_buffer_read_rate",
        QuantityKind::ByteRate,
        "local_buffer_read_rate",
        &["local_blks_read"],
    ),
    search_quantity(
        "local_buffer_dirty_rate",
        QuantityKind::ByteRate,
        "local_buffer_dirty_rate",
        &["local_blks_dirtied"],
    ),
    search_quantity(
        "local_buffer_write_rate",
        QuantityKind::ByteRate,
        "local_buffer_write_rate",
        &["local_blks_written"],
    ),
    search_quantity(
        "temp_buffer_read_rate",
        QuantityKind::ByteRate,
        "temp_buffer_read_rate",
        &["temp_blks_read"],
    ),
    search_quantity(
        "temp_buffer_write_rate",
        QuantityKind::ByteRate,
        "temp_buffer_write_rate",
        &["temp_blks_written"],
    ),
    search_quantity(
        "shared_read_time_rate",
        QuantityKind::DurationRate,
        "shared_read_time_rate",
        &["blk_read_time", "shared_blk_read_time"],
    ),
    search_quantity(
        "shared_write_time_rate",
        QuantityKind::DurationRate,
        "shared_write_time_rate",
        &["blk_write_time", "shared_blk_write_time"],
    ),
    search_quantity(
        "local_read_time_rate",
        QuantityKind::DurationRate,
        "local_read_time_rate",
        &["local_blk_read_time"],
    ),
    search_quantity(
        "local_write_time_rate",
        QuantityKind::DurationRate,
        "local_write_time_rate",
        &["local_blk_write_time"],
    ),
    search_quantity(
        "temp_read_time_rate",
        QuantityKind::DurationRate,
        "temp_read_time_rate",
        &["temp_blk_read_time"],
    ),
    search_quantity(
        "temp_write_time_rate",
        QuantityKind::DurationRate,
        "temp_write_time_rate",
        &["temp_blk_write_time"],
    ),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit",
        &["shared_blks_hit", "shared_blks_read"],
    ),
    search_quantity(
        "buffer_per_call",
        QuantityKind::Bytes,
        "buffer_per_call",
        &[
            "calls",
            "shared_blks_hit",
            "shared_blks_read",
            "shared_blks_dirtied",
            "shared_blks_written",
            "local_blks_hit",
            "local_blks_read",
            "local_blks_dirtied",
            "local_blks_written",
            "temp_blks_read",
            "temp_blks_written",
        ],
    ),
    search_quantity(
        "slow_call_rate",
        QuantityKind::CountRate,
        "slow_call_rate",
        &["slow_log_calls"],
    ),
    search_quantity(
        "exec_cv",
        QuantityKind::Scalar,
        "exec_cv",
        &["mean_time", "stddev_time"],
    ),
    search_quantity(
        "min_exec_since_reset",
        QuantityKind::Duration,
        "min_exec_since_reset",
        &["min_time"],
    ),
    search_quantity(
        "max_exec_since_reset",
        QuantityKind::Duration,
        "max_exec_since_reset",
        &["max_time"],
    ),
    search_quantity(
        "mean_exec_since_reset",
        QuantityKind::Duration,
        "mean_exec_since_reset",
        &["mean_time"],
    ),
    search_quantity(
        "stddev_exec_since_reset",
        QuantityKind::Duration,
        "stddev_exec_since_reset",
        &["stddev_time"],
    ),
];
const TABLE_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &["datname", "schemaname", "relname", "tablespace"],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("schema", &[], &["schemaname"]),
    search_string("table_name", &["table"], &["relname"]),
    search_string("tablespace", &[], &["tablespace"]),
    search_quantity(
        "size",
        QuantityKind::Bytes,
        "displayed_storage_bytes",
        &["main_fork_bytes", "toast_bytes"],
    ),
    search_quantity("table_count", QuantityKind::Count, "table_count", &[]),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit_pct",
        &[
            "heap_blks_hit",
            "heap_blks_read",
            "idx_blks_hit",
            "idx_blks_read",
            "toast_blks_hit",
            "toast_blks_read",
            "tidx_blks_hit",
            "tidx_blks_read",
        ],
    ),
    search_quantity(
        "seq_scan_rate",
        QuantityKind::CountRate,
        "seq_scan",
        &["seq_scan"],
    ),
    search_quantity(
        "change_rate",
        QuantityKind::CountRate,
        "dml_total",
        &["n_tup_ins", "n_tup_upd", "n_tup_del"],
    ),
    search_quantity(
        "autovacuum_rate",
        QuantityKind::CountRate,
        "autovacuum_count",
        &["autovacuum_count"],
    ),
    search_quantity(
        "autovacuum_mean",
        QuantityKind::Duration,
        "autovacuum_mean_ms",
        &["total_autovacuum_time", "autovacuum_count"],
    ),
    search_quantity("xid_age", QuantityKind::Count, "xid_age", &["xid_age"]),
];
const INDEX_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &[
            "datname",
            "schemaname",
            "relname",
            "indexrelname",
            "tablespace",
            "amname",
            "indexdef",
        ],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("schema", &[], &["schemaname"]),
    search_string("table_name", &["table"], &["relname"]),
    search_string("index_name", &["index"], &["indexrelname"]),
    search_string("access_method", &["method"], &["amname"]),
    search_string("definition", &[], &["indexdef"]),
    search_string("tablespace", &[], &["tablespace"]),
    search_quantity(
        "size",
        QuantityKind::Bytes,
        "main_fork_bytes",
        &["main_fork_bytes"],
    ),
    search_quantity("index_count", QuantityKind::Count, "index_count", &[]),
    search_quantity(
        "buffer_hit",
        QuantityKind::Percentage,
        "buffer_hit_pct",
        &["idx_blks_hit", "idx_blks_read"],
    ),
    search_quantity(
        "scan_rate",
        QuantityKind::CountRate,
        "idx_scan",
        &["idx_scan"],
    ),
];
const PROCESS_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["comm", "cmdline", "uid", "euid", "scope"]),
    search_string("user", &["username"], &["uid", "scope"]),
    search_string("effective_user", &["euser"], &["euid", "scope"]),
    search_id("user_id", &["uid"], &["uid"], false),
    search_id("effective_user_id", &["euid"], &["euid"], false),
    search_id("pid", &[], &["pid"], false),
    search_id("parent_pid", &["ppid"], &["ppid"], false),
    search_string("command", &["cmd"], &["comm", "cmdline"]),
    search_string("state", &[], &["state"]),
    search_quantity_aliases(
        "rss",
        &["resident_memory"],
        QuantityKind::Bytes,
        "rss",
        &["rmem_kb"],
    ),
    search_quantity_aliases(
        "vsz",
        &["virtual_memory"],
        QuantityKind::Bytes,
        "vsz",
        &["vmem_kb"],
    ),
    search_quantity("swap", QuantityKind::Bytes, "swap", &["vswap_kb"]),
    search_quantity("threads", QuantityKind::Count, "threads", &["num_threads"]),
    search_quantity(
        "cpu_cores",
        QuantityKind::Scalar,
        "cpu_cores",
        &["utime", "stime", "starttime"],
    ),
    search_quantity(
        "user_cpu_cores",
        QuantityKind::Scalar,
        "user_cpu_cores",
        &["utime", "starttime"],
    ),
    search_quantity(
        "system_cpu_cores",
        QuantityKind::Scalar,
        "system_cpu_cores",
        &["stime", "starttime"],
    ),
    search_quantity_aliases(
        "disk_read_rate",
        &["read_bytes_rate"],
        QuantityKind::ByteRate,
        "disk_read_rate",
        &["read_bytes", "starttime"],
    ),
    search_quantity_aliases(
        "disk_write_rate",
        &["write_bytes_rate"],
        QuantityKind::ByteRate,
        "disk_write_rate",
        &["write_bytes", "starttime"],
    ),
    search_quantity_aliases(
        "logical_read_rate",
        &["rchar_rate"],
        QuantityKind::ByteRate,
        "logical_read_rate",
        &["rchar", "starttime"],
    ),
    search_quantity_aliases(
        "logical_write_rate",
        &["wchar_rate"],
        QuantityKind::ByteRate,
        "logical_write_rate",
        &["wchar", "starttime"],
    ),
    search_quantity_aliases(
        "read_syscall_rate",
        &["syscr_rate"],
        QuantityKind::CountRate,
        "read_syscall_rate",
        &["syscr", "starttime"],
    ),
    search_quantity_aliases(
        "write_syscall_rate",
        &["syscw_rate"],
        QuantityKind::CountRate,
        "write_syscall_rate",
        &["syscw", "starttime"],
    ),
    search_quantity_aliases(
        "major_fault_rate",
        &["majflt_rate"],
        QuantityKind::CountRate,
        "major_fault_rate",
        &["majflt", "starttime"],
    ),
    search_quantity_aliases(
        "minor_fault_rate",
        &["minflt_rate"],
        QuantityKind::CountRate,
        "minor_fault_rate",
        &["minflt", "starttime"],
    ),
    search_quantity(
        "context_switch_rate",
        QuantityKind::CountRate,
        "context_switch_rate",
        &["nvcsw", "nivcsw", "starttime"],
    ),
    search_quantity_aliases(
        "voluntary_context_switch_rate",
        &["nvcsw_rate"],
        QuantityKind::CountRate,
        "voluntary_context_switch_rate",
        &["nvcsw", "starttime"],
    ),
    search_quantity_aliases(
        "involuntary_context_switch_rate",
        &["nivcsw_rate"],
        QuantityKind::CountRate,
        "involuntary_context_switch_rate",
        &["nivcsw", "starttime"],
    ),
    search_quantity_aliases(
        "run_delay",
        &["rundelay"],
        QuantityKind::DurationRate,
        "run_delay",
        &["rundelay_ns", "starttime"],
    ),
    search_quantity_aliases(
        "block_io_delay",
        &["blkdelay"],
        QuantityKind::DurationRate,
        "block_io_delay",
        &["blkdelay_ticks", "starttime"],
    ),
];
const ACTIVITY_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &[
            "query",
            "application_name",
            "client_addr",
            "datname",
            "usename",
        ],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_string("application", &["app"], &["application_name"]),
    search_string("client_addr", &["client"], &["client_addr"]),
    search_string("backend_type", &[], &["backend_type"]),
    search_string("state", &[], &["state"]),
    search_string("wait_event_type", &[], &["wait_event_type"]),
    search_string("wait_event", &[], &["wait_event"]),
    search_id("pid", &[], &["pid"], false),
    search_id("query_id", &[], &["query_id"], true),
    search_quantity(
        "backend_xid_age",
        QuantityKind::Count,
        "backend_xid_age",
        &["backend_xid_age"],
    ),
    search_quantity(
        "backend_xmin_age",
        QuantityKind::Count,
        "backend_xmin_age",
        &["backend_xmin_age"],
    ),
];
const LOCKS_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &["query", "datname", "usename", "lock_relname"],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("role", &["user"], &["usename"]),
    search_string("state", &[], &["state"]),
    search_string("lock_type", &["locktype"], &["lock_locktype"]),
    search_string("lock_mode", &["mode"], &["lock_mode"]),
    search_string("table_name", &["table"], &["lock_relname"]),
    search_id("pid", &[], &["pid"], false),
];
const VACUUM_SEARCH_FIELDS: &[SearchField] = &[
    search_string(
        "text",
        &["q"],
        &["datname", "relname", "schemaname", "phase"],
    ),
    search_string("database", &["db"], &["datname"]),
    search_string("schema", &[], &["schemaname"]),
    search_string("table_name", &["table"], &["relname"]),
    search_string("phase", &[], &["phase"]),
    search_string("is_autovacuum", &["autovacuum"], &["is_autovacuum"]),
    search_id("pid", &[], &["pid"], false),
    search_quantity(
        "heap_blks_total",
        QuantityKind::Count,
        "heap_blks_total",
        &["heap_blks_total"],
    ),
    search_quantity(
        "heap_blks_scanned",
        QuantityKind::Count,
        "heap_blks_scanned",
        &["heap_blks_scanned"],
    ),
    search_quantity(
        "heap_blks_vacuumed",
        QuantityKind::Count,
        "heap_blks_vacuumed",
        &["heap_blks_vacuumed"],
    ),
];
const DATABASE_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["datname"]),
    search_string("database", &["db"], &["datname"]),
    search_id("datid", &[], &["datid"], false),
    search_quantity(
        "numbackends",
        QuantityKind::Count,
        "numbackends",
        &["numbackends"],
    ),
    search_quantity(
        "xact_commit",
        QuantityKind::Count,
        "xact_commit",
        &["xact_commit"],
    ),
    search_quantity(
        "xact_rollback",
        QuantityKind::Count,
        "xact_rollback",
        &["xact_rollback"],
    ),
    search_quantity(
        "deadlocks",
        QuantityKind::Count,
        "deadlocks",
        &["deadlocks"],
    ),
    search_quantity(
        "temp_bytes",
        QuantityKind::Bytes,
        "temp_bytes",
        &["temp_bytes"],
    ),
];

const CGROUP_SEARCH_FIELDS: &[SearchField] = &[
    search_string("text", &["q"], &["cgroup_path"]),
    search_string("path", &["cgroup_path"], &["cgroup_path"]),
];
