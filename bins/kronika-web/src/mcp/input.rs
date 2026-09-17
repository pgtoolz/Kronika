//! Typed MCP arguments. Shared finder fields retain one wire contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup};
use kronika_query::snapshot::SEARCH_MAX_CLAUSES;
use kronika_query::{DEFAULT_TOP, MAX_TOP};

use super::filter::FilterInput;
use super::time::TimeSpecInput;

/// Executes ordered stored-data rankings over one half-open time window.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverviewInput {
    /// Inclusive start of the recorded interval. Accepts Unix microseconds as
    /// a JSON integer or decimal string, RFC 3339, `now`, or `now-N`.
    pub(crate) from: TimeSpecInput,
    /// Exclusive end of the recorded interval. Accepts the same time forms as
    /// `from`.
    pub(crate) to: TimeSpecInput,
    /// Ordered nonempty ranking groups. Each field expands to an independent
    /// result position; exact duplicates remain in place.
    #[schemars(length(min = 1))]
    pub(crate) rankings: Vec<OverviewRankingInput>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverviewRankingInput {
    /// Recorded logical section. `kronika_list_recorded_sections` lists valid
    /// names.
    #[schemars(length(min = 1, max = 128))]
    pub(crate) section: String,
    /// One to four numeric fields. Each field is ranked independently in the
    /// listed order, repeated names remain in place, and every emitted result
    /// identifies the section, one field, and its exact unit.
    #[schemars(length(min = 1, max = 4))]
    pub(crate) fields: Vec<String>,
    /// Maximum entities returned for this field result. It does not combine
    /// fields. Defaults to 25 when omitted.
    #[serde(default = "default_overview_top")]
    #[schemars(range(min = 1, max = MAX_TOP))]
    pub(crate) top: u64,
}

const fn default_overview_top() -> u64 {
    DEFAULT_TOP as u64
}

/// Narrows the answer to one recorded section; omit it for all of them.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetContextInput {
    /// One recorded logical section name; omit for every recorded section.
    #[serde(default)]
    pub(crate) section: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsScopeInput {
    /// Return every row except those whose recorded source is exactly `default`.
    #[default]
    NonDefault,
    /// Return every recorded `PostgreSQL` setting row.
    All,
}

/// Selects the recorded `PostgreSQL` settings included with instance facts.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetInstanceInput {
    /// `non_default` omits only rows whose recorded `source` is exactly
    /// `default`; null, missing, and unknown sources remain. `all` returns
    /// defaults too. Omit for `non_default`.
    #[serde(default)]
    pub(crate) settings: SettingsScopeInput,
}

/// Output identity for table and index tools. `object` keeps one table or
/// index identity. Other values aggregate matching objects; each metric uses
/// its own reducer.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupInput {
    /// One row per recorded table or index.
    Object,
    /// One aggregate row per database.
    Database,
    /// One aggregate row per schema.
    Schema,
    /// One aggregate row per tablespace.
    Tablespace,
}

impl From<GroupInput> for RelationGroup {
    fn from(value: GroupInput) -> Self {
        match value {
            GroupInput::Object => Self::Object,
            GroupInput::Database => Self::Database,
            GroupInput::Schema => Self::Schema,
            GroupInput::Tablespace => Self::Tablespace,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DirectionInput {
    Asc,
    Desc,
}

impl From<DirectionInput> for Order {
    fn from(value: DirectionInput) -> Self {
        match value {
            DirectionInput::Asc => Self::Asc,
            DirectionInput::Desc => Self::Desc,
        }
    }
}

/// Sorts matching rows before applying `limit`; omit for stable identity order.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SortInput {
    /// Sort token documented by the enclosing tool. Unknown tokens are
    /// rejected; a plain tool retains identity order for a known column the
    /// selected rows do not expose.
    pub(crate) field: String,
    /// `asc` puts the lowest non-null value first; `desc` puts the highest.
    /// Nulls remain last.
    pub(crate) direction: DirectionInput,
}

// Keep schema names and field documentation specific to each tool while sharing
// the point, filtering, ordering, and row-limit envelope. Do not flatten a common
// struct here: that would change the advertised schemas and unknown-field rules.
macro_rules! finder_inputs {
    ($(
        $(#[$doc:meta])*
        $name:ident {
            $(group: $group:ty,)?
            $(#[$filter_doc:meta])*
            filters,
            $(#[$sort_doc:meta])*
            sort,
        }
    )*) => {$(
        $(#[$doc])*
        #[derive(Debug, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct $name {
            /// Recorded point to read. If omitted, uses the latest recorded timestamp
            /// across the store. Accepts Unix microseconds as a JSON integer or decimal
            /// string, RFC 3339, `now`, or `now-N`.
            #[serde(default)]
            pub(crate) at: Option<TimeSpecInput>,
            $(
                /// Identity level of each returned row: `object`, `database`, `schema`, or
                /// `tablespace`.
                pub(crate) group: $group,
            )?
            $(#[$filter_doc])*
            #[serde(default)]
            #[schemars(length(max = SEARCH_MAX_CLAUSES))]
            pub(crate) filters: Vec<FilterInput>,
            $(#[$sort_doc])*
            #[serde(default)]
            pub(crate) sort: Option<SortInput>,
            /// Maximum rows returned. If additional rows matched, `truncated` is true.
            #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
            pub(crate) limit: u32,
        }
    )*};
}

finder_inputs! {
    /// Input for `kronika_find_postgresql_tables`.
    TablesInput {
        group: GroupInput,
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`, `schema`, `table_name`, `tablespace`. Quantity fields
        /// (`gt` or `lt`): `size` (bytes), `table_count` and `xid_age` (count),
        /// `buffer_hit` (percentage points), `seq_scan_rate`, `change_rate`, and
        /// `autovacuum_rate` (count/s), `autovacuum_mean` (ms). Empty or omitted
        /// matches all rows.
        filters,
        /// Returned field available for the selected group, such as `seq_scan`,
        /// `n_live_tup`, `dead_pct`, `buffer_hit_pct`,
        /// `displayed_storage_bytes`, or `xid_age`. Invalid fields are rejected.
        /// Omit for identity order.
        sort,
    }
    /// Input for `kronika_find_postgresql_indexes`.
    IndexesInput {
        group: GroupInput,
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`, `schema`, `table_name`, `index_name`, `access_method`,
        /// `definition`, `tablespace`. Quantity fields (`gt` or `lt`): `size`
        /// (bytes), `index_count` (count), `buffer_hit` (percentage points), and
        /// `scan_rate` (count/s). Empty or omitted matches all rows.
        filters,
        /// Returned field available for the selected group, such as `idx_scan`,
        /// `idx_tup_read`, `tuples_per_scan`, `main_fork_bytes`,
        /// `buffer_hit_pct`, or `state_severity`. Invalid fields are rejected.
        /// Omit for identity order.
        sort,
    }
    /// Input for `kronika_find_postgresql_activity`.
    ActivityInput {
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`, `role`, `application`, `client_addr`, `backend_type`,
        /// `state`, `wait_event_type`, `wait_event`. Identifier fields (`eq` or `in`):
        /// `pid`, `query_id`. Quantity fields (`gt` or `lt`): `backend_xid_age`
        /// and `backend_xmin_age` (count). Empty or omitted matches all rows.
        filters,
        /// Returned field, such as `pid`, `datname`, `state`,
        /// `backend_xid_age`, `backend_xmin_age`, or `query_start`. Filter aliases
        /// are not sort aliases; unknown names are rejected.
        sort,
    }
    /// Input for `kronika_find_postgresql_locks`. Every returned row includes
    /// `blocked_by`, a list of direct blocker PIDs; it is not filterable.
    LocksInput {
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`, `role`, `state`, `lock_type`, `lock_mode`, `table_name`.
        /// Identifier field (`eq` or `in`): `pid`. Empty or omitted matches all
        /// rows.
        filters,
        /// Returned scalar field, such as `pid`, `datname`, `state`,
        /// `lock_locktype`, `lock_mode`, `lock_relname`, or `waitstart`. Filter
        /// aliases are not sort aliases; unknown names are rejected.
        sort,
    }
    /// Input for `kronika_find_postgresql_vacuum`.
    VacuumInput {
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`, `schema`, `table_name`, `phase`, `is_autovacuum`; use the
        /// string `"true"` or `"false"` for `is_autovacuum`. Identifier field
        /// (`eq` or `in`): `pid`. Quantity fields (`gt` or `lt`): `heap_blks_total`,
        /// `heap_blks_scanned`, `heap_blks_vacuumed` (count). Empty or omitted
        /// matches all rows.
        filters,
        /// Returned field, such as `pid`, `datname`, `schemaname`,
        /// `relname`, `phase`, `heap_blks_scanned`, or `heap_blks_vacuumed`.
        /// Filter aliases are not sort aliases. An unavailable name leaves rows in
        /// identity order.
        sort,
    }
    /// Input for `kronika_find_postgresql_databases`.
    DatabasesInput {
        /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
        /// `database`. Identifier field (`eq` or `in`): `datid`. Quantity fields
        /// (`gt` or `lt`): `numbackends` (count); `xact_commit`, `xact_rollback`, and
        /// `deadlocks` compare counter delta per microsecond; `temp_bytes` compares
        /// byte delta per microsecond. Returned versions of the latter four are
        /// per-second rates. Empty or omitted matches all rows.
        filters,
        /// Returned field, such as `datid`, `datname`, `numbackends`,
        /// `xact_commit`, `deadlocks`, `temp_bytes`, or `active_time`. Cumulative
        /// fields sort by interval rate. Filter aliases are not sort aliases;
        /// unknown names are rejected.
        sort,
    }
    /// Input for `kronika_find_postgresql_statements`.
    StatementsInput {
        /// AND-only predicates. Text: `text`, `database`, `role`; identifier:
        /// `query_id`; count/s: `call_rate`, `row_rate`, `plan_rate`; ms/s:
        /// `exec_time_rate`, `planning_time_rate`, `shared_read_time_rate`,
        /// `shared_write_time_rate`, `local_read_time_rate`,
        /// `local_write_time_rate`, `temp_read_time_rate`,
        /// `temp_write_time_rate`; ms: `mean_exec`, `min_exec_since_reset`,
        /// `max_exec_since_reset`, `mean_exec_since_reset`,
        /// `stddev_exec_since_reset`; unitless: `rows_per_call`, `exec_cv`;
        /// percentage points: `planning_share`, `buffer_hit`; bytes/s:
        /// `shared_buffer_hit_rate`, `shared_buffer_read_rate`,
        /// `shared_buffer_dirty_rate`, `shared_buffer_write_rate`,
        /// `local_buffer_hit_rate`, `local_buffer_read_rate`,
        /// `local_buffer_dirty_rate`, `local_buffer_write_rate`,
        /// `temp_buffer_read_rate`, `temp_buffer_write_rate`, `wal_rate`; bytes:
        /// `buffer_per_call`, `wal_per_call`. Use `eq`/`in`/`contains` for text,
        /// `eq`/`in` for the identifier, and `gt`/`lt` for quantities. Empty or
        /// omitted matches all rows.
        filters,
        /// Returned field, such as `calls`, `total_exec_time`, `rows`,
        /// `shared_blks_read`, or `wal_bytes`, or one of the seven returned
        /// `derived_*` names (`derived_hit_fraction` and
        /// `derived_plan_time_fraction` rank by their 0-100 renderings — the
        /// same order). Unknown names are rejected; omit for identity order.
        sort,
    }
    /// Input for `kronika_find_postgresql_plans`.
    PlansInput {
        /// AND-only predicates. Text: `text`, `database`, `role`; identifiers:
        /// `query_id`, `plan_id`; count: `calls`; count/s: `call_rate`, `row_rate`,
        /// `slow_call_rate`; ms/s: `exec_time_rate`, `planning_time_rate`,
        /// `shared_read_time_rate`, `shared_write_time_rate`,
        /// `local_read_time_rate`, `local_write_time_rate`, `temp_read_time_rate`,
        /// `temp_write_time_rate`; ms: `mean_exec`, `min_exec_since_reset`,
        /// `max_exec_since_reset`, `mean_exec_since_reset`,
        /// `stddev_exec_since_reset`; unitless: `rows_per_call`, `exec_cv`;
        /// percentage points: `planning_share`, `buffer_hit`; bytes/s:
        /// `shared_buffer_hit_rate`, `shared_buffer_read_rate`,
        /// `shared_buffer_dirty_rate`, `shared_buffer_write_rate`,
        /// `local_buffer_hit_rate`, `local_buffer_read_rate`,
        /// `local_buffer_dirty_rate`, `local_buffer_write_rate`,
        /// `temp_buffer_read_rate`, `temp_buffer_write_rate`; bytes:
        /// `buffer_per_call`. Use `eq`/`in`/`contains` for text, `eq`/`in` for
        /// identifiers, and `gt`/`lt` for quantities. Empty or omitted matches all
        /// rows.
        filters,
        /// Returned field, such as `calls`, `total_time`, `rows`, or
        /// `shared_blks_read`, or one of the seven returned `derived_*` names.
        /// `calls` sorts by its interval rate although the returned field is an
        /// exact cumulative count. Unknown names are rejected; omit for
        /// identity order.
        sort,
    }
    /// Input for `kronika_find_processes`.
    ProcessesInput {
        /// AND-only predicates. Text: `text`, `user`, `effective_user`, `command`,
        /// `state`; identifiers: `user_id`, `effective_user_id`, `pid`,
        /// `parent_pid`; bytes: `rss`, `vsz`, `swap`; count: `threads`; unitless
        /// CPU cores: `cpu_cores`, `user_cpu_cores`, `system_cpu_cores`; bytes/s:
        /// `disk_read_rate`, `disk_write_rate`, `logical_read_rate`,
        /// `logical_write_rate`; count/s: `read_syscall_rate`,
        /// `write_syscall_rate`, `major_fault_rate`, `minor_fault_rate`,
        /// `context_switch_rate`, `voluntary_context_switch_rate`,
        /// `involuntary_context_switch_rate`; ms/s: `run_delay`,
        /// `block_io_delay`. Use `eq`/`in`/`contains` for text, `eq`/`in` for
        /// identifiers, and `gt`/`lt` for quantities. Empty or omitted matches all
        /// rows.
        filters,
        /// Returned field, such as `pid`, `comm`, `rmem_kb`, `vmem_kb`,
        /// `num_threads`, `utime`, `read_bytes`, or `rundelay_ns`. Filter aliases
        /// (`rss`, `vsz`, `threads`, and rate names) and virtual fields are not
        /// sort aliases; unknown names are rejected. Omit for identity order.
        sort,
    }
}

/// Opaque reference emitted by a result that supports full row detail.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RowDetailInput {
    /// Opaque reference emitted by Kronika. Copy it unchanged to
    /// `kronika_get_row_detail`.
    #[schemars(length(
        min = 1,
        max = kronika_query::DETAIL_REF_MAX_ENCODED_BYTES
    ))]
    pub(crate) detail_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventsRepresentation {
    Groups,
    Occurrences,
}

impl EventsRepresentation {
    pub(crate) const fn into_query(self) -> kronika_query::EventsRepresentation {
        match self {
            Self::Groups => kronika_query::EventsRepresentation::Groups,
            Self::Occurrences => kronika_query::EventsRepresentation::Occurrences,
        }
    }
}

/// Reads selected recorded event sections over a half-open time window.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsInput {
    /// Recorded sections to read: `pg_log_errors`, `pg_log_checkpoints`,
    /// `pg_log_autovacuum`, `pg_log_slow_queries`, `pg_log_lock_waits`,
    /// `pg_log_temp_files`, `pg_log_lifecycle`, `pgbouncer_events`. Omit or
    /// use null for every source valid for the representation; an empty array
    /// reads none. Repeats are removed after their first occurrence.
    #[serde(default)]
    pub(crate) sources: Option<Vec<String>>,
    /// Inclusive start of the recorded interval. Accepts Unix microseconds as
    /// a JSON integer or decimal string, RFC 3339, `now`, or `now-N`.
    pub(crate) from: TimeSpecInput,
    /// Exclusive end of the recorded interval. Accepts the same time forms as
    /// `from`; the interval may not exceed one hour.
    pub(crate) to: TimeSpecInput,
    /// `groups` returns merged event summaries. `occurrences` returns
    /// individual recorded events.
    #[serde(default = "default_events_representation")]
    pub(crate) representation: EventsRepresentation,
    /// Maximum events or groups returned. If more matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

const fn default_events_representation() -> EventsRepresentation {
    EventsRepresentation::Groups
}
