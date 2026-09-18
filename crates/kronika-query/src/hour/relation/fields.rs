//! Relation output fields, grouping keys, and physical projections.

use super::{Kind, RelationField, RelationKind};

use crate::{QueryError, RelationGroup};

pub(super) const TABLES: &str = "pg_stat_user_tables";
pub(super) const INDEXES: &str = "pg_stat_user_indexes";

pub(super) const TABLE_RATES: &[&str] = &[
    "seq_scan",
    "seq_tup_read",
    "idx_scan",
    "idx_tup_fetch",
    "n_tup_ins",
    "n_tup_upd",
    "n_tup_del",
    "n_tup_hot_upd",
    "n_tup_newpage_upd",
    "vacuum_count",
    "autovacuum_count",
    "analyze_count",
    "autoanalyze_count",
    "total_vacuum_time",
    "total_autovacuum_time",
    "total_analyze_time",
    "total_autoanalyze_time",
    "heap_blks_read",
    "heap_blks_hit",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];

pub(super) const TABLE_STRUCTURAL_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];

pub(super) const TABLE_GAUGES: &[&str] = &[
    "n_live_tup",
    "n_dead_tup",
    "n_mod_since_analyze",
    "n_ins_since_vacuum",
    "main_fork_bytes",
    "toast_bytes",
    "toast_n_live_tup",
    "toast_n_dead_tup",
    "reltuples",
];

pub(super) const TABLE_STRUCTURAL_GAUGES: &[&str] =
    &["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"];
pub(super) const TABLE_MAXIMA: &[&str] = &["xid_age", "mxid_age"];
pub(super) const TABLE_TIMESTAMPS: &[&str] = &[
    "last_vacuum",
    "last_autovacuum",
    "last_analyze",
    "last_autoanalyze",
    "last_seq_scan",
    "last_idx_scan",
    "toast_last_autovacuum",
];

pub(super) const INDEX_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_read",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
];
pub(super) const INDEX_GAUGES: &[&str] = &["main_fork_bytes"];
pub(super) const INDEX_TIMESTAMPS: &[&str] = &["last_idx_scan"];
pub(super) const INDEX_FLAGS: &[&str] = &[
    "indisunique",
    "indisprimary",
    "indisvalid",
    "indisexclusion",
    "indisready",
];

const TABLE_OBJECT_FIELDS: &[RelationField] = &[
    field("relname", Kind::Text, None),
    field("tablespace", Kind::Text, None),
    count_field("table_count"),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    rate_field("tuple_throughput"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    field("last_seq_scan_never", Kind::Boolean, None),
    field("last_idx_scan_never", Kind::Boolean, None),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("dml_total"),
    percent_field("insert_share_pct"),
    percent_field("update_share_pct"),
    percent_field("delete_share_pct"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    count_field("n_live_tup"),
    count_field("n_dead_tup"),
    count_field("n_mod_since_analyze"),
    count_field("n_ins_since_vacuum"),
    count_field("reltuples"),
    percent_field("dead_pct"),
    percent_field("hot_pct"),
    percent_field("new_page_pct"),
    rate_field("vacuum_count"),
    rate_field("autovacuum_count"),
    rate_field("analyze_count"),
    rate_field("autoanalyze_count"),
    number_field_with_unit("vacuum_mean_ms", "milliseconds"),
    number_field_with_unit("autovacuum_mean_ms", "milliseconds"),
    number_field_with_unit("analyze_mean_ms", "milliseconds"),
    number_field_with_unit("autoanalyze_mean_ms", "milliseconds"),
    timestamp_field("last_vacuum"),
    timestamp_field("last_autovacuum"),
    timestamp_field("last_analyze"),
    timestamp_field("last_autoanalyze"),
    timestamp_field("last_seq_scan"),
    timestamp_field("last_idx_scan"),
    timestamp_field("toast_last_autovacuum"),
    integer_field("main_fork_bytes", Some("bytes")),
    integer_field("toast_bytes", Some("bytes")),
    integer_field("displayed_storage_bytes", Some("bytes")),
    percent_field("toast_share_pct"),
    count_field("toast_n_live_tup"),
    count_field("toast_n_dead_tup"),
    percent_field("toast_dead_pct"),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("heap_buffer_hit_pct"),
    percent_field("index_buffer_hit_pct"),
    percent_field("toast_buffer_hit_pct"),
    percent_field("tidx_buffer_hit_pct"),
    percent_field("buffer_hit_pct"),
    integer_field("xid_age", None),
    integer_field("mxid_age", None),
];

const TABLE_AGGREGATE_FIELDS: &[RelationField] = &[
    count_field("table_count"),
    rate_field("seq_scan"),
    rate_field("seq_tup_read"),
    rate_field("idx_scan"),
    rate_field("idx_tup_fetch"),
    percent_field("sequential_share_pct"),
    rate_field("tuple_throughput"),
    number_field("seq_tuples_per_scan"),
    number_field("idx_tuples_per_scan"),
    rate_field("n_tup_ins"),
    rate_field("n_tup_upd"),
    rate_field("n_tup_del"),
    rate_field("dml_total"),
    percent_field("insert_share_pct"),
    percent_field("update_share_pct"),
    percent_field("delete_share_pct"),
    rate_field("n_tup_hot_upd"),
    rate_field("n_tup_newpage_upd"),
    count_field("n_live_tup"),
    count_field("n_dead_tup"),
    count_field("n_mod_since_analyze"),
    count_field("n_ins_since_vacuum"),
    count_field("reltuples"),
    percent_field("dead_pct"),
    percent_field("hot_pct"),
    percent_field("new_page_pct"),
    rate_field("vacuum_count"),
    rate_field("autovacuum_count"),
    rate_field("analyze_count"),
    rate_field("autoanalyze_count"),
    number_field_with_unit("vacuum_mean_ms", "milliseconds"),
    number_field_with_unit("autovacuum_mean_ms", "milliseconds"),
    number_field_with_unit("analyze_mean_ms", "milliseconds"),
    number_field_with_unit("autoanalyze_mean_ms", "milliseconds"),
    timestamp_field("last_vacuum_oldest"),
    timestamp_field("last_vacuum_latest"),
    count_field("last_vacuum_never_count"),
    timestamp_field("last_autovacuum_oldest"),
    timestamp_field("last_autovacuum_latest"),
    count_field("last_autovacuum_never_count"),
    timestamp_field("last_analyze_oldest"),
    timestamp_field("last_analyze_latest"),
    count_field("last_analyze_never_count"),
    timestamp_field("last_autoanalyze_oldest"),
    timestamp_field("last_autoanalyze_latest"),
    count_field("last_autoanalyze_never_count"),
    timestamp_field("last_seq_scan_oldest"),
    timestamp_field("last_seq_scan_latest"),
    count_field("last_seq_scan_never_count"),
    timestamp_field("last_idx_scan_oldest"),
    timestamp_field("last_idx_scan_latest"),
    count_field("last_idx_scan_never_count"),
    timestamp_field("toast_last_autovacuum_oldest"),
    timestamp_field("toast_last_autovacuum_latest"),
    count_field("toast_last_autovacuum_never_count"),
    integer_field("main_fork_bytes", Some("bytes")),
    integer_field("toast_bytes", Some("bytes")),
    integer_field("displayed_storage_bytes", Some("bytes")),
    percent_field("toast_share_pct"),
    count_field("toast_n_live_tup"),
    count_field("toast_n_dead_tup"),
    percent_field("toast_dead_pct"),
    rate_field("heap_blks_read"),
    rate_field("heap_blks_hit"),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    rate_field("toast_blks_read"),
    rate_field("toast_blks_hit"),
    rate_field("tidx_blks_read"),
    rate_field("tidx_blks_hit"),
    percent_field("heap_buffer_hit_pct"),
    percent_field("index_buffer_hit_pct"),
    percent_field("toast_buffer_hit_pct"),
    percent_field("tidx_buffer_hit_pct"),
    percent_field("buffer_hit_pct"),
    integer_field("xid_age", None),
    integer_field("mxid_age", None),
];

const INDEX_OBJECT_FIELDS: &[RelationField] = &[
    field("indexrelname", Kind::Text, None),
    field("relname", Kind::Text, None),
    field("relid", Kind::Id, None),
    field("tablespace", Kind::Text, None),
    field("amname", Kind::Text, None),
    count_field("index_count"),
    rate_field("idx_scan"),
    rate_field("idx_tup_read"),
    rate_field("idx_tup_fetch"),
    number_field("tuples_per_scan"),
    number_field("fetches_per_scan"),
    integer_field("main_fork_bytes", Some("bytes")),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    percent_field("buffer_hit_pct"),
    timestamp_field("last_idx_scan"),
    field("last_idx_scan_never", Kind::Boolean, None),
    field("no_scans", Kind::Boolean, None),
    field("indisunique", Kind::Boolean, None),
    field("indisprimary", Kind::Boolean, None),
    field("indisvalid", Kind::Boolean, None),
    field("indisexclusion", Kind::Boolean, None),
    field("indisready", Kind::Boolean, None),
    integer_field("state_severity", None),
];

const INDEX_AGGREGATE_FIELDS: &[RelationField] = &[
    count_field("index_count"),
    rate_field("idx_scan"),
    rate_field("idx_tup_read"),
    rate_field("idx_tup_fetch"),
    number_field("tuples_per_scan"),
    number_field("fetches_per_scan"),
    integer_field("main_fork_bytes", Some("bytes")),
    rate_field("idx_blks_read"),
    rate_field("idx_blks_hit"),
    percent_field("buffer_hit_pct"),
    timestamp_field("last_idx_scan_oldest"),
    timestamp_field("last_idx_scan_latest"),
    count_field("last_idx_scan_never_count"),
    count_field("no_scan_count"),
    count_field("known_scan_count"),
    count_field("invalid_count"),
    count_field("unready_count"),
    count_field("unique_count"),
    count_field("primary_count"),
    count_field("exclusion_count"),
    integer_field("state_severity", None),
];

impl RelationKind {
    /// Resolve one supported relation section.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::BadFilter`] when `name` is not a relation section.
    pub fn from_name(name: &str) -> Result<Self, QueryError> {
        match name {
            TABLES => Ok(Self::Tables),
            INDEXES => Ok(Self::Indexes),
            _ => Err(QueryError::BadFilter("group".to_owned())),
        }
    }

    pub(super) const fn count_field(self) -> &'static str {
        match self {
            Self::Tables => "table_count",
            Self::Indexes => "index_count",
        }
    }

    /// Public result fields for this relation kind and grouping.
    #[must_use]
    pub fn fields(self, group: RelationGroup) -> Vec<RelationField> {
        let base = match (self, group) {
            (Self::Tables, RelationGroup::Object) => TABLE_OBJECT_FIELDS,
            (
                Self::Tables,
                RelationGroup::Database | RelationGroup::Schema | RelationGroup::Tablespace,
            ) => TABLE_AGGREGATE_FIELDS,
            (Self::Indexes, RelationGroup::Object) => INDEX_OBJECT_FIELDS,
            (
                Self::Indexes,
                RelationGroup::Database | RelationGroup::Schema | RelationGroup::Tablespace,
            ) => INDEX_AGGREGATE_FIELDS,
        };
        if group != RelationGroup::Tablespace {
            return base.to_vec();
        }
        let mut fields = Vec::with_capacity(base.len().saturating_add(1));
        fields.push(field("tablespace", Kind::Text, None));
        fields.extend_from_slice(base);
        fields
    }

    /// Physical columns needed to calculate the requested semantic fields.
    #[must_use]
    pub fn physical_fields(self, group: RelationGroup, names: &[String]) -> Vec<String> {
        physical_fields(self, group, names)
    }

    /// Whether a field is available as a result metric or grouping key.
    #[must_use]
    pub fn sort_field_known(self, group: RelationGroup, name: &str) -> bool {
        self.fields(group).iter().any(|field| field.name == name)
            || key_fields(self, group).contains(&name)
    }

    /// Registry logical section name for this relation kind.
    #[must_use]
    pub const fn logical_name(self) -> &'static str {
        match self {
            Self::Tables => TABLES,
            Self::Indexes => INDEXES,
        }
    }
}

impl RelationField {
    /// Public field name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Wire-level field kind name.
    #[must_use]
    pub const fn kind_name(self) -> &'static str {
        kind_name(self.kind)
    }

    /// Optional wire-level unit.
    #[must_use]
    pub const fn unit(self) -> Option<&'static str> {
        self.unit
    }
}

const fn field(name: &'static str, kind: Kind, unit: Option<&'static str>) -> RelationField {
    RelationField { name, kind, unit }
}

const fn rate_field(name: &'static str) -> RelationField {
    field(name, Kind::Number, Some("per_second"))
}

const fn percent_field(name: &'static str) -> RelationField {
    field(name, Kind::Number, Some("percent"))
}

const fn number_field(name: &'static str) -> RelationField {
    field(name, Kind::Number, None)
}

const fn number_field_with_unit(name: &'static str, unit: &'static str) -> RelationField {
    field(name, Kind::Number, Some(unit))
}

const fn integer_field(name: &'static str, unit: Option<&'static str>) -> RelationField {
    field(name, Kind::Integer, unit)
}

const fn count_field(name: &'static str) -> RelationField {
    integer_field(name, Some("count"))
}

const fn timestamp_field(name: &'static str) -> RelationField {
    field(name, Kind::Timestamp, None)
}

/// Validate and normalize requested relation output fields.
///
/// # Errors
///
/// Returns a query validation error for an invalid section or field name.
pub fn output_fields(
    sections: &[String],
    group: RelationGroup,
    requested: &[String],
) -> Result<Vec<String>, QueryError> {
    let [logical_name] = sections else {
        return Err(QueryError::BadFilter("group".to_owned()));
    };
    let kind = RelationKind::from_name(logical_name)?;
    let available = kind.fields(group);
    if requested.is_empty() {
        return Ok(available
            .iter()
            .map(|field| field.name.to_owned())
            .collect());
    }
    let keys = key_fields(kind, group);
    for name in requested {
        if !kind.sort_field_known(group, name) {
            return Err(QueryError::NoSuchColumn(name.clone()));
        }
    }
    let mut output = Vec::with_capacity(requested.len());
    for name in requested {
        if !keys.contains(&name.as_str()) && !output.contains(name) {
            output.push(name.clone());
        }
    }
    Ok(output)
}
pub(super) fn physical_fields(
    kind: RelationKind,
    group: RelationGroup,
    fields: &[String],
) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    let keys: &[&str] = match group {
        RelationGroup::Database => &["datid", "datname"],
        RelationGroup::Schema => &["datid", "datname", "schemaname"],
        RelationGroup::Tablespace => &["datid", "tablespace_oid", "tablespace"],
        RelationGroup::Object => &[],
    };
    names.extend(keys.iter().copied());
    for field in fields {
        let dependencies: &[&str] = match (kind, field.as_str()) {
            (RelationKind::Tables, "sequential_share_pct") => &["seq_scan", "idx_scan"],
            (RelationKind::Tables, "tuple_throughput") => &["seq_tup_read", "idx_tup_fetch"],
            (RelationKind::Tables, "seq_tuples_per_scan") => &["seq_tup_read", "seq_scan"],
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => &["idx_tup_fetch", "idx_scan"],
            (
                RelationKind::Tables,
                "dml_total" | "insert_share_pct" | "update_share_pct" | "delete_share_pct",
            ) => &["n_tup_ins", "n_tup_upd", "n_tup_del"],
            (RelationKind::Tables, "dead_pct") => &["n_live_tup", "n_dead_tup"],
            (RelationKind::Tables, "hot_pct") => &["n_tup_hot_upd", "n_tup_upd"],
            (RelationKind::Tables, "new_page_pct") => &["n_tup_newpage_upd", "n_tup_upd"],
            (RelationKind::Tables, "displayed_storage_bytes" | "toast_share_pct") => {
                &["main_fork_bytes", "toast_bytes"]
            }
            (RelationKind::Tables, "toast_dead_pct") => {
                &["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"]
            }
            (RelationKind::Tables, "heap_buffer_hit_pct") => &["heap_blks_read", "heap_blks_hit"],
            (RelationKind::Tables, "index_buffer_hit_pct")
            | (RelationKind::Indexes, "buffer_hit_pct") => &["idx_blks_read", "idx_blks_hit"],
            (RelationKind::Tables, "toast_buffer_hit_pct") => {
                &["toast_blks_read", "toast_blks_hit"]
            }
            (RelationKind::Tables, "tidx_buffer_hit_pct") => &["tidx_blks_read", "tidx_blks_hit"],
            (RelationKind::Tables, "buffer_hit_pct") => &[
                "heap_blks_read",
                "heap_blks_hit",
                "idx_blks_read",
                "idx_blks_hit",
                "toast_blks_read",
                "toast_blks_hit",
                "tidx_blks_read",
                "tidx_blks_hit",
            ],
            (RelationKind::Tables, "vacuum_mean_ms") => &["total_vacuum_time", "vacuum_count"],
            (RelationKind::Tables, "autovacuum_mean_ms") => {
                &["total_autovacuum_time", "autovacuum_count"]
            }
            (RelationKind::Tables, "analyze_mean_ms") => &["total_analyze_time", "analyze_count"],
            (RelationKind::Tables, "autoanalyze_mean_ms") => {
                &["total_autoanalyze_time", "autoanalyze_count"]
            }
            (RelationKind::Indexes, "tuples_per_scan") => &["idx_tup_read", "idx_scan"],
            (RelationKind::Indexes, "no_scan_count" | "known_scan_count") => &["idx_scan"],
            (RelationKind::Indexes, "state_severity") => &["indisvalid", "indisready"],
            (RelationKind::Indexes, "invalid_count") => &["indisvalid"],
            (RelationKind::Indexes, "unready_count") => &["indisready"],
            (RelationKind::Indexes, "unique_count") => &["indisunique"],
            (RelationKind::Indexes, "primary_count") => &["indisprimary"],
            (RelationKind::Indexes, "exclusion_count") => &["indisexclusion"],
            _ => {
                let timestamp = ["_oldest", "_latest", "_never_count"]
                    .iter()
                    .find_map(|suffix| field.strip_suffix(suffix));
                if let Some(timestamp) = timestamp {
                    names.insert(timestamp);
                    if timestamp == "toast_last_autovacuum" {
                        names.insert("toast_bytes");
                    }
                } else if (kind == RelationKind::Tables
                    && (TABLE_RATES.contains(&field.as_str())
                        || TABLE_GAUGES.contains(&field.as_str())
                        || TABLE_MAXIMA.contains(&field.as_str())))
                    || (kind == RelationKind::Indexes
                        && (INDEX_RATES.contains(&field.as_str())
                            || INDEX_GAUGES.contains(&field.as_str())
                            || INDEX_FLAGS.contains(&field.as_str())))
                {
                    names.insert(field);
                }
                &[]
            }
        };
        names.extend(dependencies.iter().copied());
    }
    names.into_iter().map(ToOwned::to_owned).collect()
}
pub(super) const fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Number | Kind::Integer => "number",
        Kind::Id => "id",
        Kind::Timestamp => "timestamp",
        Kind::Boolean => "bool",
        Kind::Text => "text",
    }
}

/// Stable key fields for one relation kind and grouping.
#[must_use]
pub const fn key_fields(kind: RelationKind, group: RelationGroup) -> &'static [&'static str] {
    match (kind, group) {
        (_, RelationGroup::Database) => &["datid", "datname"],
        (_, RelationGroup::Schema) => &["datid", "datname", "schemaname"],
        (_, RelationGroup::Tablespace) => &["tablespace_oid"],
        (RelationKind::Tables, RelationGroup::Object) => {
            &["datid", "datname", "schemaname", "relid", "relname"]
        }
        (RelationKind::Indexes, RelationGroup::Object) => &[
            "datid",
            "datname",
            "schemaname",
            "relid",
            "relname",
            "indexrelid",
            "indexrelname",
        ],
    }
}
