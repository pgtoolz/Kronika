//! Tool names, descriptions, and input schemas returned by `tools/list`.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup};
use kronika_query::snapshot::SEARCH_MAX_CLAUSES;
use kronika_query::{DEFAULT_TOP, MAX_TOP};

use super::filter::FilterInput;
use super::instance::InstanceOutput;
use super::semantics::FinderOutput;
use super::time::TimeSpecInput;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_rank_metrics";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_list_recorded_sections";
pub(crate) const GET_INSTANCE_TOOL: &str = "kronika_get_instance";
pub(crate) const FIND_POSTGRESQL_TABLES_TOOL: &str = "kronika_find_postgresql_tables";
pub(crate) const FIND_POSTGRESQL_INDEXES_TOOL: &str = "kronika_find_postgresql_indexes";
pub(crate) const FIND_POSTGRESQL_ACTIVITY_TOOL: &str = "kronika_find_postgresql_activity";
pub(crate) const FIND_POSTGRESQL_LOCKS_TOOL: &str = "kronika_find_postgresql_locks";
pub(crate) const FIND_POSTGRESQL_VACUUM_TOOL: &str = "kronika_find_postgresql_vacuum";
pub(crate) const FIND_POSTGRESQL_DATABASES_TOOL: &str = "kronika_find_postgresql_databases";
pub(crate) const FIND_POSTGRESQL_STATEMENTS_TOOL: &str = "kronika_find_postgresql_statements";
pub(crate) const FIND_POSTGRESQL_PLANS_TOOL: &str = "kronika_find_postgresql_plans";
pub(crate) const FIND_PROCESSES_TOOL: &str = "kronika_find_processes";
pub(crate) const GET_ROW_DETAIL_TOOL: &str = "kronika_get_row_detail";
pub(crate) const FIND_EVENTS_TOOL: &str = "kronika_find_events";

pub(crate) const SERVER_INSTRUCTIONS: &str = "Kronika returns observability data that it has already recorded. It never reads current state directly from a host or database.\n\n`kronika_list_recorded_sections` lists the recorded time bounds and the available sections, fields, units, and sources. Finder tools read one recorded point in time. `kronika_rank_metrics` and `kronika_find_events` read the half-open interval `[from,to)`.\n\n`now` is resolved when the request is handled and does not indicate the timestamp of the newest recorded observation.\n\nA `detail_ref` is opaque. Pass it unchanged to `kronika_get_row_detail`.";

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

/// Stable catalog envelope returned by `kronika_list_recorded_sections`.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RecordedSectionsOutput {
    /// Earliest recorded timestamp as decimal Unix microseconds, or null when
    /// the store is empty.
    recorded_from: Option<String>,
    /// Exclusive upper bound as decimal Unix microseconds, or null when the
    /// store is empty.
    recorded_to: Option<String>,
    /// Recorded logical sections in stable name order.
    sections: Vec<RecordedSectionOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RecordedSectionOutput {
    /// Logical section name accepted by section-aware tools.
    logical_name: String,
    /// Recorded source family, or null when the registry has none.
    source_family: Option<String>,
    /// Recorded row count as an exact decimal integer string.
    rows: String,
    /// Recorded encoded byte count as an exact decimal integer string.
    bytes: String,
    /// Public fields in stable name order.
    fields: Vec<RecordedFieldOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RecordedFieldOutput {
    /// Public field name.
    name: String,
    /// Metric class such as `counter`, `gauge`, or `identity`.
    class: String,
    /// Field unit, or null when the field has no unit.
    unit: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeatmapBatchResult {
    results: Vec<HeatmapItemResult>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeatmapItemResult {
    ranking: NormalizedRanking,
    coverage: HeatmapCoverage,
    class: String,
    /// Summary reduction: `sum`, `max`, or `mean`.
    summary: String,
    unit: Option<String>,
    entities: Vec<HeatmapEntity>,
    totals_total: Option<f64>,
    others_total: Option<f64>,
    entity_count: String,
    out_of_order: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NormalizedRanking {
    section: String,
    #[schemars(length(min = 1, max = 1))]
    fields: Vec<String>,
    top: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeatmapCoverage {
    state: CoverageState,
    window_rows: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[expect(
    dead_code,
    reason = "schema-only variants mirror serialized query results"
)]
enum CoverageState {
    Data,
    NoData,
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeatmapEntity {
    identity: std::collections::BTreeMap<String, Value>,
    labels: std::collections::BTreeMap<String, Value>,
    detail_ref: String,
    total: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "representation", rename_all = "snake_case")]
#[expect(
    dead_code,
    reason = "schema-only variants mirror serialized query results"
)]
enum EventsResult {
    Groups {
        groups: Vec<EventGroup>,
        truncated: bool,
    },
    Occurrences {
        occurrences: Vec<EventOccurrence>,
        truncated: bool,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[expect(
    dead_code,
    reason = "schema-only variants mirror serialized query results"
)]
enum EventTier {
    Critical,
    Notable,
    Routine,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct EventGroup {
    key: String,
    section: String,
    tier: EventTier,
    label: Option<String>,
    count: f64,
    first_ts: i64,
    last_ts: i64,
    representative_ts: i64,
    minutes: Vec<f64>,
    stat: EventStat,
    #[serde(rename = "detail_ref")]
    detail_ref: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind")]
#[expect(
    dead_code,
    reason = "schema-only variants mirror serialized query results"
)]
enum EventStat {
    #[serde(rename = "pg.errors")]
    Errors {
        severity: f64,
        category: Option<f64>,
        sqlstate: Option<String>,
        database: Option<String>,
        username: Option<String>,
    },
    #[serde(rename = "pg.slow", rename_all = "camelCase")]
    Slow {
        max_ms: f64,
        total_ms: f64,
        threshold_ms: Option<f64>,
    },
    #[serde(rename = "pg.autovacuum", rename_all = "camelCase")]
    Autovacuum {
        analyze: bool,
        runs: usize,
        total_ms: Option<f64>,
        tuples_removed: Option<f64>,
        tuples_dead: Option<f64>,
    },
    #[serde(rename = "pg.checkpoints", rename_all = "camelCase")]
    Checkpoints {
        completes: usize,
        timed: usize,
        requested: usize,
        max_sync_ms: Option<f64>,
        buffers: Option<f64>,
    },
    #[serde(rename = "pg.checkpoint_warning", rename_all = "camelCase")]
    CheckpointWarning { seconds_apart: Option<f64> },
    #[serde(rename = "pg.locks", rename_all = "camelCase")]
    Locks {
        holders: Option<String>,
        acquired: bool,
        waiters: usize,
        max_ms: Option<f64>,
        targets: Vec<String>,
    },
    #[serde(rename = "pg.lifecycle")]
    Lifecycle {
        lifecycle: f64,
        pid: Option<f64>,
        signal: Option<f64>,
        mode: Option<String>,
    },
    #[serde(rename = "pgbouncer.events")]
    Pgbouncer {
        level: f64,
        database: Option<String>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
struct EventOccurrence {
    #[serde(flatten)]
    fields: Map<String, Value>,
    source: String,
    detail_ref: String,
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

/// Input for `kronika_find_postgresql_tables`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TablesInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// Identity level of each returned row: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `tablespace`. Quantity fields
    /// (`gt` or `lt`): `size` (bytes), `table_count` and `xid_age` (count),
    /// `buffer_hit` (percentage points), `seq_scan_rate`, `change_rate`, and
    /// `autovacuum_rate` (count/s), `autovacuum_mean` (ms). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field available for the selected group, such as `seq_scan`,
    /// `n_live_tup`, `dead_pct`, `buffer_hit_pct`,
    /// `displayed_storage_bytes`, or `xid_age`. Invalid fields are rejected.
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_indexes`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexesInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// Identity level of each returned row: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `index_name`, `access_method`,
    /// `definition`, `tablespace`. Quantity fields (`gt` or `lt`): `size`
    /// (bytes), `index_count` (count), `buffer_hit` (percentage points), and
    /// `scan_rate` (count/s). Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field available for the selected group, such as `idx_scan`,
    /// `idx_tup_read`, `tuples_per_scan`, `main_fork_bytes`,
    /// `buffer_hit_pct`, or `state_severity`. Invalid fields are rejected.
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_activity`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `role`, `application`, `client_addr`, `backend_type`,
    /// `state`, `wait_event_type`, `wait_event`. Identifier fields (`eq` or `in`):
    /// `pid`, `query_id`. Quantity fields (`gt` or `lt`): `backend_xid_age`
    /// and `backend_xmin_age` (count). Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `pid`, `datname`, `state`,
    /// `backend_xid_age`, `backend_xmin_age`, or `query_start`. Filter aliases
    /// are not sort aliases; unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_locks`. Every returned row includes
/// `blocked_by`, a list of direct blocker PIDs; it is not filterable.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocksInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `role`, `state`, `lock_type`, `lock_mode`, `table_name`.
    /// Identifier field (`eq` or `in`): `pid`. Empty or omitted matches all
    /// rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned scalar field, such as `pid`, `datname`, `state`,
    /// `lock_locktype`, `lock_mode`, `lock_relname`, or `waitstart`. Filter
    /// aliases are not sort aliases; unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_vacuum`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct VacuumInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `phase`, `is_autovacuum`; use the
    /// string `"true"` or `"false"` for `is_autovacuum`. Identifier field
    /// (`eq` or `in`): `pid`. Quantity fields (`gt` or `lt`): `heap_blks_total`,
    /// `heap_blks_scanned`, `heap_blks_vacuumed` (count). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `pid`, `datname`, `schemaname`,
    /// `relname`, `phase`, `heap_blks_scanned`, or `heap_blks_vacuumed`.
    /// Filter aliases are not sort aliases. An unavailable name leaves rows in
    /// identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_databases`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabasesInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`. Identifier field (`eq` or `in`): `datid`. Quantity fields
    /// (`gt` or `lt`): `numbackends` (count); `xact_commit`, `xact_rollback`, and
    /// `deadlocks` compare counter delta per microsecond; `temp_bytes` compares
    /// byte delta per microsecond. Returned versions of the latter four are
    /// per-second rates. Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `datid`, `datname`, `numbackends`,
    /// `xact_commit`, `deadlocks`, `temp_bytes`, or `active_time`. Cumulative
    /// fields sort by interval rate. Filter aliases are not sort aliases;
    /// unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_statements`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatementsInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
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
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `calls`, `total_exec_time`, `rows`,
    /// `shared_blks_read`, or `wal_bytes`, or one of the seven returned
    /// `derived_*` names (`derived_hit_fraction` and
    /// `derived_plan_time_fraction` rank by their 0-100 renderings — the
    /// same order). Unknown names are rejected; omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_plans`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlansInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
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
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `calls`, `total_time`, `rows`, or
    /// `shared_blks_read`, or one of the seven returned `derived_*` names.
    /// `calls` sorts by its interval rate although the returned field is an
    /// exact cumulative count. Unknown names are rejected; omit for
    /// identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_processes`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessesInput {
    /// Recorded point to read. If omitted, uses the latest recorded timestamp
    /// across the store. Accepts Unix microseconds as a JSON integer or decimal
    /// string, RFC 3339, `now`, or `now-N`.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
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
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field, such as `pid`, `comm`, `rmem_kb`, `vmem_kb`,
    /// `num_threads`, `utime`, `read_bytes`, or `rundelay_ns`. Filter aliases
    /// (`rss`, `vsz`, `threads`, and rate names) and virtual fields are not
    /// sort aliases; unknown names are rejected. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows returned. If additional rows matched, `truncated` is true.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
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

pub(crate) fn tools() -> Vec<Tool> {
    entry_tools()
        .into_iter()
        .chain(std::iter::once(instance_tool()))
        .chain(relation_tools())
        .chain(postgresql_plain_tools())
        .chain(ratio_tools())
        .chain(tail_tools())
        .collect()
}

/// Metric ranking and recorded-section catalog tools.
fn entry_tools() -> [Tool; 2] {
    [overview_tool(), context_tool()]
}

fn overview_tool() -> Tool {
    Tool::new(
        OVERVIEW_TOOL,
        "Ranks recorded numeric fields over the half-open interval `[from,to)`. \
         Each requested field produces a separate result in request order. Counter \
         totals are non-negative changes across the interval; gauge totals are \
         maximum recorded values. Use `kronika_list_recorded_sections` when a \
         section or field name is unknown.",
        schema_object::<OverviewInput>(),
    )
    .with_raw_output_schema(opaque_output_schema::<HeatmapBatchResult>())
}

fn context_tool() -> Tool {
    Tool::new(
        GET_CONTEXT_TOOL,
        "Lists the recorded time bounds and sections available in Kronika. Each \
         section includes its source, row and byte counts, and public fields with \
         their class and unit. Pass `section` to return one section.",
        schema_object::<GetContextInput>(),
    )
    .with_raw_output_schema(output_schema_object::<RecordedSectionsOutput>())
}

/// Instance metadata tool.
fn instance_tool() -> Tool {
    Tool::new(
        GET_INSTANCE_TOOL,
        "Returns the latest recorded host metadata and PostgreSQL settings. Host \
         metadata and settings have separate recorded timestamps. Settings whose \
         recorded source is `default` are omitted unless `settings` is `\"all\"`.",
        schema_object::<GetInstanceInput>(),
    )
    .with_raw_output_schema(output_schema_object::<InstanceOutput>())
}

/// `PostgreSQL` relation tools.
fn relation_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Finds or groups recorded PostgreSQL table statistics at `at`. Filters \
             are applied before aggregation. `group` selects table, database, schema, \
             or tablespace; each metric uses the reducer stated in its field \
             description. Aggregated rows do not have `detail_ref`.",
            schema_object::<TablesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Finds or groups recorded PostgreSQL index statistics at `at`. Filters \
             are applied before aggregation. `group` selects index, database, schema, \
             or tablespace; each metric uses the reducer stated in its field \
             description. Aggregated rows do not have `detail_ref`.",
            schema_object::<IndexesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
    ]
}

/// Process, row-detail, and event tools.
fn tail_tools() -> [Tool; 3] {
    [
        Tool::new(
            FIND_PROCESSES_TOOL,
            "Finds Linux process observations at `at`, then applies filters, sorting, \
             and `limit`. Rates are derived between compatible observations of the \
             same process; unavailable rates are null. Command lines are available \
             through `detail_ref`.",
            schema_object::<ProcessesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            GET_ROW_DETAIL_TOOL,
            "Returns the stored row addressed by `detail_ref`. Pass a reference \
             emitted by Kronika unchanged. Long text is returned as \
             `{stored_text, full_len, truncated, sha256}`.",
            schema_object::<RowDetailInput>(),
        )
        .with_raw_output_schema(row_detail_output_schema()),
        Tool::new(
            FIND_EVENTS_TOOL,
            "Finds recorded PostgreSQL and PgBouncer events in the half-open \
             interval `[from,to)`. `groups` returns merged summaries; `occurrences` \
             returns individual events. `limit` is applied after merging or grouping. \
             Raw event payloads are available through `detail_ref`.",
            schema_object::<EventsInput>(),
        )
        .with_raw_output_schema(opaque_output_schema::<EventsResult>()),
    ]
}

/// `PostgreSQL` point finders.
fn postgresql_plain_tools() -> [Tool; 4] {
    [
        Tool::new(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Finds PostgreSQL backend activity at `at`, including state, waits, \
             query identifiers, and transaction or query timestamps. Query text is \
             available through `detail_ref`.",
            schema_object::<ActivityInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Finds PostgreSQL backends participating in direct lock waits at `at`. \
             `blocked_by` contains direct blocker PIDs; an empty list marks a root \
             or blocker-only row, and PID `0` denotes a prepared transaction.",
            schema_object::<LocksInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Finds PostgreSQL backends recorded as running `VACUUM` near `at`, \
             including progress counts, dead-tuple storage, and delay time. An empty \
             result means no vacuum observation was selected around that point.",
            schema_object::<VacuumInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Finds recorded PostgreSQL statistics for each database at `at`. \
             Cumulative counts, bytes, and times are returned as interval rates; \
             `numbackends` is a recorded count. Missing predecessors and counter \
             resets produce null rates.",
            schema_object::<DatabasesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
    ]
}

/// `PostgreSQL` statement and plan point finders.
fn ratio_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            "Finds recorded `pg_stat_statements` rows at `at`. Cumulative values \
             are returned as interval rates; latency statistics remain recorded \
             gauges. Derived fields expose per-call values and fractions defined in \
             their field descriptions. Query text is available through `detail_ref`.",
            schema_object::<StatementsInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Finds recorded `pg_store_plans` rows at `at`. `calls` is the recorded \
             cumulative count and `calls_per_second` is its interval rate; other \
             cumulative values are returned as rates. Plan text is available through \
             `detail_ref`.",
            schema_object::<PlansInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
    ]
}

/// Serializes a Schemars schema into rmcp's `Arc<JsonObject>` without binding
/// this module to Schemars schema types.
fn schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let schema = schemars::schema_for!(T);
    let value = serde_json::to_value(schema).expect("schema serializes to JSON");
    let object = value.as_object().expect("schema is a JSON object").clone();
    Arc::new(object)
}

/// Output schemas describe serialization, so nullable fields that are always
/// emitted remain required even though they accept JSON null.
fn output_schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let value = serde_json::to_value(schema).expect("schema serializes to JSON");
    let object = value.as_object().expect("schema is a JSON object").clone();
    Arc::new(object)
}

/// Row fields depend on the referenced section, so only the common envelope
/// and long-text representation can be described statically.
fn row_detail_output_schema() -> Arc<JsonObject> {
    Arc::new(
        json!({
            "type": "object",
            "description": "One recorded row. Property names and scalar value types depend on the section. Long text values are objects with stored_text, full_len, truncated, and sha256.",
            "additionalProperties": {
                "description": "A stored row field. Long text uses {stored_text:string, full_len:decimal-string, truncated:boolean, sha256:string|null}."
            }
        })
        .as_object()
        .expect("row-detail output schema is an object")
        .clone(),
    )
}

/// Adapts typed result schemas to the stable opaque MCP detail boundary.
fn opaque_output_schema<T: JsonSchema>() -> Arc<JsonObject> {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).expect("schema serializes to JSON");
    strip_schema_descriptions(&mut value);
    if let Some(definitions) = value.get_mut("$defs").and_then(Value::as_object_mut) {
        definitions.remove("DetailLocator");
    }
    rewrite_detail_schema(&mut value);
    let object = value.as_object_mut().expect("schema is a JSON object");
    object.insert("type".to_owned(), json!("object"));
    Arc::new(object.clone())
}

fn strip_schema_descriptions(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("description");
            for child in object.values_mut() {
                strip_schema_descriptions(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                strip_schema_descriptions(child);
            }
        }
        _ => {}
    }
}

fn rewrite_detail_schema(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.get("format").and_then(Value::as_str) == Some("uint") {
                object.remove("format");
            }
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                let had_locator = properties.remove("detail_locator").is_some();
                if had_locator || properties.contains_key("detail_ref") {
                    properties.insert(
                        "detail_ref".to_owned(),
                        json!({
                            "description": "Opaque server-produced row-detail reference; copy it unchanged.",
                            "type": "string",
                        }),
                    );
                }
            }
            if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                for name in required.iter_mut() {
                    if name == "detail_locator" {
                        *name = json!("detail_ref");
                    }
                }
            }
            for child in object.values_mut() {
                rewrite_detail_schema(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite_detail_schema(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
