//! Grouped relation histories for `PostgreSQL` tables and indexes.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{ColumnClass, contract, logical_section_name};
use serde_json::{Map, Value, json};

use crate::exact_product::compare_products;
use crate::projection::{Plan, chunk_dictionary, plans};
use crate::render::record;
use crate::{
    DataRequest, DatasetSegment, Filter, HourSeriesRequest, QueryDataset, QueryError, QuerySink,
    RelationGroup, SegmentRequest, Window,
};

const HISTORY_CHUNK_ROWS: usize = 1_024;

type CounterReadings = BTreeMap<&'static str, Cell>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum IdentityCell {
    Null,
    I16(i16),
    I32(i32),
    I64(i64),
    Ts(i64),
    U32(u32),
    U64(u64),
    F64(u64),
    Bool(bool),
    StrId(u64),
    ListI32(Vec<i32>),
}

#[derive(Clone, Copy)]
enum OrderedNumber {
    Integer(i128),
    Float(f64),
}

impl OrderedNumber {
    #[expect(
        clippy::cast_precision_loss,
        reason = "integer counter deltas are converted only after exact subtraction"
    )]
    const fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

const TABLES: &str = "pg_stat_user_tables";
const INDEXES: &str = "pg_stat_user_indexes";

const TABLE_RATES: &[&str] = &[
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

const TABLE_STRUCTURAL_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
    "toast_blks_read",
    "toast_blks_hit",
    "tidx_blks_read",
    "tidx_blks_hit",
];

const TABLE_GAUGES: &[&str] = &[
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

const TABLE_STRUCTURAL_GAUGES: &[&str] = &["toast_bytes", "toast_n_live_tup", "toast_n_dead_tup"];
const TABLE_MAXIMA: &[&str] = &["xid_age", "mxid_age"];
const TABLE_TIMESTAMPS: &[&str] = &[
    "last_vacuum",
    "last_autovacuum",
    "last_analyze",
    "last_autoanalyze",
    "last_seq_scan",
    "last_idx_scan",
    "toast_last_autovacuum",
];

const INDEX_RATES: &[&str] = &[
    "idx_scan",
    "idx_tup_read",
    "idx_tup_fetch",
    "idx_blks_read",
    "idx_blks_hit",
];
const INDEX_GAUGES: &[&str] = &["main_fork_bytes"];
const INDEX_TIMESTAMPS: &[&str] = &["last_idx_scan"];
const INDEX_FLAGS: &[&str] = &[
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

/// Supported relation query families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// User-table statistics.
    Tables,
    /// User-index statistics.
    Indexes,
}

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

    const fn count_field(self) -> &'static str {
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

/// One field in the stable relation result contract.
#[derive(Debug, Clone, Copy)]
pub struct RelationField {
    name: &'static str,
    kind: Kind,
    unit: Option<&'static str>,
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

#[derive(Debug, Clone, Copy)]
enum Kind {
    Number,
    Id,
    Integer,
    Timestamp,
    Boolean,
    Text,
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

/// Opaque stable identity of one relation group.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupKey(GroupKeyValue);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeyValue {
    Database {
        datid: u32,
        datname: String,
    },
    Schema {
        datid: u32,
        datname: String,
        schemaname: String,
    },
    Tablespace {
        tablespace_oid: u32,
    },
    Table {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
    },
    Index {
        datid: u32,
        datname: String,
        schemaname: String,
        relid: u32,
        relname: String,
        indexrelid: u32,
        indexrelname: String,
    },
}

impl GroupKey {
    /// Decode the stable relation grouping key from one projected row.
    ///
    /// # Errors
    ///
    /// Returns a query decoding error when a referenced dictionary value is invalid.
    pub fn from_row(
        kind: RelationKind,
        group: RelationGroup,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<Option<Self>, QueryError> {
        if group == RelationGroup::Tablespace {
            return Ok(unsigned_cell(row.get("tablespace_oid"))
                .map(|tablespace_oid| Self(GroupKeyValue::Tablespace { tablespace_oid })));
        }
        let (Some(datid), Some(datname)) = (
            unsigned_cell(row.get("datid")),
            text_cell(row.get("datname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if group == RelationGroup::Database {
            return Ok(Some(Self(GroupKeyValue::Database { datid, datname })));
        }
        let Some(schemaname) = text_cell(row.get("schemaname"), dictionary)? else {
            return Ok(None);
        };
        if group == RelationGroup::Schema {
            return Ok(Some(Self(GroupKeyValue::Schema {
                datid,
                datname,
                schemaname,
            })));
        }
        let (Some(relid), Some(relname)) = (
            unsigned_cell(row.get("relid")),
            text_cell(row.get("relname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        if kind == RelationKind::Tables {
            return Ok(Some(Self(GroupKeyValue::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            })));
        }
        let (Some(indexrelid), Some(indexrelname)) = (
            unsigned_cell(row.get("indexrelid")),
            text_cell(row.get("indexrelname"), dictionary)?,
        ) else {
            return Ok(None);
        };
        Ok(Some(Self(GroupKeyValue::Index {
            datid,
            datname,
            schemaname,
            relid,
            relname,
            indexrelid,
            indexrelname,
        })))
    }

    /// Render the grouping key using the existing JSON contract.
    #[must_use]
    pub fn json(&self, kind: RelationKind, group: RelationGroup) -> Value {
        match self.clone().for_group(kind, group).0 {
            GroupKeyValue::Database { datid, datname } => json!({
                "datid": datid.to_string(),
                "datname": datname,
            }),
            GroupKeyValue::Schema {
                datid,
                datname,
                schemaname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
            }),
            GroupKeyValue::Tablespace { tablespace_oid } => {
                json!({ "tablespace_oid": tablespace_oid.to_string() })
            }
            GroupKeyValue::Table {
                datid,
                datname,
                schemaname,
                relid,
                relname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
            }),
            GroupKeyValue::Index {
                datid,
                datname,
                schemaname,
                relid,
                relname,
                indexrelid,
                indexrelname,
            } => json!({
                "datid": datid.to_string(),
                "datname": datname,
                "schemaname": schemaname,
                "relid": relid.to_string(),
                "relname": relname,
                "indexrelid": indexrelid.to_string(),
                "indexrelname": indexrelname,
            }),
        }
    }

    fn for_group(self, _kind: RelationKind, group: RelationGroup) -> Self {
        if group == RelationGroup::Object {
            return self;
        }
        Self(match group {
            RelationGroup::Object => unreachable!("object grouping returned above"),
            RelationGroup::Database => match self.0 {
                GroupKeyValue::Table { datid, datname, .. }
                | GroupKeyValue::Index { datid, datname, .. } => {
                    GroupKeyValue::Database { datid, datname }
                }
                key => key,
            },
            RelationGroup::Schema => match self.0 {
                GroupKeyValue::Table {
                    datid,
                    datname,
                    schemaname,
                    ..
                }
                | GroupKeyValue::Index {
                    datid,
                    datname,
                    schemaname,
                    ..
                } => GroupKeyValue::Schema {
                    datid,
                    datname,
                    schemaname,
                },
                key => key,
            },
            RelationGroup::Tablespace => match self.0 {
                key @ GroupKeyValue::Tablespace { .. } => key,
                _ => unreachable!("tablespace keys are formed directly from physical rows"),
            },
        })
    }

    /// Text value of a named key component, when present.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match (&self.0, name) {
            (
                GroupKeyValue::Database { datname, .. }
                | GroupKeyValue::Schema { datname, .. }
                | GroupKeyValue::Table { datname, .. }
                | GroupKeyValue::Index { datname, .. },
                "datname",
            ) => Some(datname),
            (
                GroupKeyValue::Schema { schemaname, .. }
                | GroupKeyValue::Table { schemaname, .. }
                | GroupKeyValue::Index { schemaname, .. },
                "schemaname",
            ) => Some(schemaname),
            (
                GroupKeyValue::Table { relname, .. } | GroupKeyValue::Index { relname, .. },
                "relname",
            ) => Some(relname),
            (GroupKeyValue::Index { indexrelname, .. }, "indexrelname") => Some(indexrelname),
            _ => None,
        }
    }

    /// Numeric or text metric represented by a named key component.
    #[must_use]
    #[allow(
        clippy::unnested_or_patterns,
        reason = "each tuple arm keeps the requested field name attached to its variants"
    )]
    pub fn metric(&self, name: &str) -> Option<Metric> {
        match (&self.0, name) {
            (
                GroupKeyValue::Database { datid, .. } | GroupKeyValue::Schema { datid, .. },
                "datid",
            )
            | (GroupKeyValue::Table { datid, .. } | GroupKeyValue::Index { datid, .. }, "datid") => {
                Some(Metric::integer(i128::from(*datid)))
            }
            (
                GroupKeyValue::Database { datname, .. } | GroupKeyValue::Schema { datname, .. },
                "datname",
            )
            | (
                GroupKeyValue::Table { datname, .. } | GroupKeyValue::Index { datname, .. },
                "datname",
            ) => Some(Metric::text(datname.clone())),
            (GroupKeyValue::Schema { schemaname, .. }, "schemaname")
            | (
                GroupKeyValue::Table { schemaname, .. } | GroupKeyValue::Index { schemaname, .. },
                "schemaname",
            ) => Some(Metric::text(schemaname.clone())),
            (GroupKeyValue::Tablespace { tablespace_oid }, "tablespace_oid") => {
                Some(Metric::integer(i128::from(*tablespace_oid)))
            }
            (GroupKeyValue::Table { relid, .. } | GroupKeyValue::Index { relid, .. }, "relid") => {
                Some(Metric::integer(i128::from(*relid)))
            }
            (
                GroupKeyValue::Table { relname, .. } | GroupKeyValue::Index { relname, .. },
                "relname",
            ) => Some(Metric::text(relname.clone())),
            (GroupKeyValue::Index { indexrelid, .. }, "indexrelid") => {
                Some(Metric::integer(i128::from(*indexrelid)))
            }
            (GroupKeyValue::Index { indexrelname, .. }, "indexrelname") => {
                Some(Metric::text(indexrelname.clone()))
            }
            _ => None,
        }
    }

    const fn is_tablespace(&self) -> bool {
        matches!(&self.0, GroupKeyValue::Tablespace { .. })
    }
}

/// Stable physical source coordinates retained by the relation reducer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelationSource {
    segment_id: i64,
    context_index: usize,
    ordinal: u64,
    type_id: u32,
    timestamp: i64,
}

impl RelationSource {
    /// Bind a row to its stable source coordinates.
    #[must_use]
    pub const fn new(
        segment_id: i64,
        context_index: usize,
        ordinal: u64,
        type_id: u32,
        timestamp: i64,
    ) -> Self {
        Self {
            segment_id,
            context_index,
            ordinal,
            type_id,
            timestamp,
        }
    }

    /// Segment containing the source row.
    #[must_use]
    pub const fn segment_id(self) -> i64 {
        self.segment_id
    }

    /// Selected context index used by snapshot cursors.
    #[must_use]
    pub const fn context_index(self) -> usize {
        self.context_index
    }

    /// Source-row ordinal.
    #[must_use]
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }

    /// Physical type identifier.
    #[must_use]
    pub const fn type_id(self) -> u32 {
        self.type_id
    }

    /// Source-row timestamp in microseconds.
    #[must_use]
    pub const fn timestamp(self) -> i64 {
        self.timestamp
    }
}

#[derive(Clone, Copy, Default)]
enum Availability {
    #[default]
    Empty,
    Value(OrderedNumber),
    Unavailable,
}

impl Availability {
    fn add(&mut self, input: Input) {
        match (*self, input) {
            (Self::Unavailable, _) | (_, Input::Unavailable) => *self = Self::Unavailable,
            (Self::Empty | Self::Value(_), Input::Neutral) => {}
            (Self::Empty, Input::Value(value)) => *self = Self::Value(value),
            (Self::Value(left), Input::Value(right)) => {
                *self = add_ordered(Some(left), right).map_or(Self::Unavailable, Self::Value);
            }
        }
    }

    const fn value(self) -> Option<OrderedNumber> {
        match self {
            Self::Value(value) => Some(value),
            Self::Empty | Self::Unavailable => None,
        }
    }

    fn merge(&mut self, other: Self) {
        self.add(match other {
            Self::Empty => Input::Neutral,
            Self::Value(value) => Input::Value(value),
            Self::Unavailable => Input::Unavailable,
        });
    }
}

#[derive(Clone, Copy)]
enum Input {
    Value(OrderedNumber),
    Neutral,
    Unavailable,
}

#[expect(
    variant_size_differences,
    reason = "exact rational rates avoid heap allocation and preserve ordering"
)]
#[derive(Clone, Copy, Debug)]
enum RateValue {
    /// A non-negative rate represented exactly as units per microsecond.
    Exact { numerator: u128, denominator: u128 },
    /// Units per microsecond when the input counter is floating point or exact
    /// rational accumulation exceeds `u128`.
    Float(f64),
}

impl RateValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an interval of 2^52 microseconds is 142 years"
    )]
    fn from_delta(delta: OrderedNumber, elapsed: i64) -> Option<Self> {
        let denominator = u128::try_from(elapsed).ok().filter(|value| *value > 0)?;
        match delta {
            OrderedNumber::Integer(value) => {
                let numerator = u128::try_from(value).ok()?;
                Some(Self::exact(numerator, denominator))
            }
            OrderedNumber::Float(value) => {
                let rate = value / elapsed as f64;
                rate.is_finite().then_some(Self::Float(rate))
            }
        }
    }

    const fn exact(numerator: u128, denominator: u128) -> Self {
        let divisor = greatest_common_divisor(numerator, denominator);
        Self::Exact {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        }
    }

    fn add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Exact {
                    numerator: left_numerator,
                    denominator: left_denominator,
                },
                Self::Exact {
                    numerator: right_numerator,
                    denominator: right_denominator,
                },
            ) => {
                let common = greatest_common_divisor(left_denominator, right_denominator);
                let left_scale = right_denominator / common;
                let right_scale = left_denominator / common;
                let exact = left_numerator
                    .checked_mul(left_scale)
                    .and_then(|left| {
                        right_numerator
                            .checked_mul(right_scale)
                            .and_then(|right| left.checked_add(right))
                    })
                    .zip(left_denominator.checked_mul(left_scale))
                    .map(|(numerator, denominator)| Self::exact(numerator, denominator));
                exact.or_else(|| Self::float_sum(self, other))
            }
            _ => Self::float_sum(self, other),
        }
    }

    fn float_sum(self, other: Self) -> Option<Self> {
        let value = self.per_microsecond() + other.per_microsecond();
        value.is_finite().then_some(Self::Float(value))
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "floating output is produced only after exact rational accumulation"
    )]
    fn per_microsecond(self) -> f64 {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => numerator as f64 / denominator as f64,
            Self::Float(value) => value,
        }
    }

    fn per_second(self) -> f64 {
        self.per_microsecond() * 1_000_000.0
    }
}

const fn greatest_common_divisor(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

#[derive(Clone, Copy, Default)]
struct RateAggregate {
    value: Option<RateValue>,
    unavailable: bool,
}

impl RateAggregate {
    fn add(&mut self, input: Input, elapsed: Option<i64>) {
        if self.unavailable {
            return;
        }
        match input {
            Input::Neutral => {}
            Input::Unavailable => self.unavailable = true,
            Input::Value(delta) => {
                let Some(value) = elapsed.and_then(|elapsed| RateValue::from_delta(delta, elapsed))
                else {
                    self.unavailable = true;
                    return;
                };
                self.value = self.value.map_or(Some(value), |known| known.add(value));
                if self.value.is_none() {
                    self.unavailable = true;
                }
            }
        }
    }

    fn metric(self) -> Option<Metric> {
        if self.unavailable {
            None
        } else {
            self.value.map(Metric::rate)
        }
    }

    const fn value(self) -> Option<RateValue> {
        if self.unavailable { None } else { self.value }
    }

    fn merge(&mut self, other: Self) {
        if self.unavailable || other.unavailable {
            self.unavailable = true;
            self.value = None;
            return;
        }
        if let Some(value) = other.value {
            self.value = self.value.map_or(Some(value), |known| known.add(value));
            if self.value.is_none() {
                self.unavailable = true;
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct MaximumAggregate {
    maximum: Option<i128>,
    unavailable: bool,
}

impl MaximumAggregate {
    fn add(&mut self, present: bool, stored: Option<&Cell>) {
        if !present {
            self.unavailable = true;
            return;
        }
        match stored {
            Some(Cell::Null) => {}
            Some(cell) => match integer_cell(Some(cell)) {
                Some(value) => {
                    self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
                }
                None => self.unavailable = true,
            },
            None => self.unavailable = true,
        }
    }

    const fn exact(self) -> Option<i128> {
        if self.unavailable { None } else { self.maximum }
    }

    fn merge(&mut self, other: Self) {
        self.unavailable |= other.unavailable;
        if let Some(value) = other.maximum {
            self.maximum = Some(self.maximum.map_or(value, |known| known.max(value)));
        }
    }
}

#[derive(Clone, Copy, Default)]
struct TimestampAggregate {
    applicable: u64,
    unavailable: bool,
    oldest: Option<i64>,
    latest: Option<i64>,
    never: u64,
}

#[derive(Clone, Copy)]
enum TimestampObservation<'a> {
    Unavailable,
    NotApplicable,
    Stored(Option<&'a Cell>),
}

impl TimestampAggregate {
    fn add(&mut self, observation: TimestampObservation<'_>) {
        let stored = match observation {
            TimestampObservation::Unavailable => {
                self.unavailable = true;
                return;
            }
            TimestampObservation::NotApplicable => return,
            TimestampObservation::Stored(stored) => stored,
        };
        self.applicable = self.applicable.saturating_add(1);
        match stored {
            Some(Cell::Ts(value)) => {
                self.oldest = Some(self.oldest.map_or(*value, |oldest| oldest.min(*value)));
                self.latest = Some(self.latest.map_or(*value, |latest| latest.max(*value)));
            }
            Some(Cell::Null) => self.never = self.never.saturating_add(1),
            _ => self.unavailable = true,
        }
    }

    const fn exact(self) -> bool {
        self.applicable > 0 && !self.unavailable
    }

    fn merge(&mut self, other: Self) {
        self.applicable = self.applicable.saturating_add(other.applicable);
        self.unavailable |= other.unavailable;
        self.oldest = match (self.oldest, other.oldest) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.latest = match (self.latest, other.latest) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        self.never = self.never.saturating_add(other.never);
    }
}

#[derive(Clone, Copy, Default)]
struct BoolAggregate {
    known: u64,
    truthy: u64,
    unavailable: bool,
}

impl BoolAggregate {
    fn add(&mut self, stored: Option<&Cell>) {
        match stored {
            Some(Cell::Bool(value)) => {
                self.known = self.known.saturating_add(1);
                self.truthy = self.truthy.saturating_add(u64::from(*value));
            }
            _ => self.unavailable = true,
        }
    }

    fn add_scan(&mut self, input: Input) {
        match input {
            Input::Value(value) => self.add(Some(&Cell::Bool(number_is_zero(value)))),
            Input::Neutral | Input::Unavailable => self.unavailable = true,
        }
    }

    const fn exact(self) -> bool {
        self.known > 0 && !self.unavailable
    }

    const fn merge(&mut self, other: Self) {
        self.known = self.known.saturating_add(other.known);
        self.truthy = self.truthy.saturating_add(other.truthy);
        self.unavailable |= other.unavailable;
    }
}

/// Incremental reducer for one relation grouping key.
#[derive(Clone)]
pub struct RelationAggregate {
    key: GroupKey,
    count: u64,
    source: RelationSource,
    from: Option<i64>,
    to: Option<i64>,
    rates: BTreeMap<&'static str, RateAggregate>,
    gauges: BTreeMap<&'static str, Availability>,
    maxima: BTreeMap<&'static str, MaximumAggregate>,
    timestamps: BTreeMap<&'static str, TimestampAggregate>,
    flags: BTreeMap<&'static str, BoolAggregate>,
    texts: BTreeMap<&'static str, String>,
    tablespace_label_timestamp: Option<i64>,
    identifiers: BTreeMap<&'static str, i128>,
    no_scans: BoolAggregate,
    state_severity: Option<i128>,
}

impl std::fmt::Debug for RelationAggregate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelationAggregate")
            .field("key", &self.key)
            .field("source", &self.source)
            .field("sample_from", &self.from)
            .field("sample_to", &self.to)
            .finish_non_exhaustive()
    }
}

impl RelationAggregate {
    /// Start an empty reducer for one stable grouping key.
    #[must_use]
    pub fn new(key: GroupKey, source: RelationSource) -> Self {
        Self {
            key,
            count: 0,
            source,
            from: None,
            to: None,
            rates: BTreeMap::new(),
            gauges: BTreeMap::new(),
            maxima: BTreeMap::new(),
            timestamps: BTreeMap::new(),
            flags: BTreeMap::new(),
            texts: BTreeMap::new(),
            tablespace_label_timestamp: None,
            identifiers: BTreeMap::new(),
            no_scans: BoolAggregate::default(),
            state_severity: None,
        }
    }

    /// Stable grouping key accumulated by this reducer.
    #[must_use]
    pub const fn key(&self) -> &GroupKey {
        &self.key
    }

    /// Deterministic source coordinate retained for cursor tie-breaking.
    #[must_use]
    pub const fn source(&self) -> RelationSource {
        self.source
    }

    /// Earliest contributing sample boundary.
    #[must_use]
    pub const fn sample_from(&self) -> Option<i64> {
        self.from
    }

    /// Latest contributing sample boundary.
    #[must_use]
    pub const fn sample_to(&self) -> Option<i64> {
        self.to
    }

    /// Text value retained from physical rows, when present.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.texts.get(name).map(String::as_str)
    }

    /// Add one physical relation row to this reducer.
    ///
    /// # Errors
    ///
    /// Returns a query decoding error when a referenced dictionary value is invalid.
    #[expect(
        clippy::too_many_arguments,
        reason = "one physical object carries its validated interval and source coordinates"
    )]
    pub fn add(
        &mut self,
        kind: RelationKind,
        plan: &Plan,
        row: &Row,
        before: Option<&BTreeMap<&'static str, Cell>>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
        source: RelationSource,
    ) -> Result<(), QueryError> {
        self.count = self.count.saturating_add(1);
        self.source = self.source.min(source);
        self.to = Some(
            self.to
                .map_or(source.timestamp, |to| to.max(source.timestamp)),
        );
        if let Some(from) = elapsed.and_then(|elapsed| source.timestamp.checked_sub(elapsed)) {
            self.from = Some(self.from.map_or(from, |oldest| oldest.min(from)));
        }
        if self.key.is_tablespace()
            && let Some(label) = text_cell(row.get("tablespace"), dictionary)?
        {
            let replace = self.tablespace_label_timestamp.is_none_or(|timestamp| {
                source.timestamp > timestamp
                    || source.timestamp == timestamp
                        && self
                            .texts
                            .get("tablespace")
                            .is_none_or(|current| label.as_bytes() < current.as_bytes())
            });
            if replace {
                self.tablespace_label_timestamp = Some(source.timestamp);
                self.texts.insert("tablespace", label);
            }
        }
        match kind {
            RelationKind::Tables => self.add_table(plan, row, before, elapsed, dictionary)?,
            RelationKind::Indexes => self.add_index(plan, row, before, elapsed, dictionary)?,
        }
        Ok(())
    }

    fn merge(&mut self, other: &Self) {
        self.count = self.count.saturating_add(other.count);
        self.source = self.source.min(other.source);
        self.from = match (self.from, other.from) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        self.to = match (self.to, other.to) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        for (&name, rate) in &other.rates {
            self.rates.entry(name).or_default().merge(*rate);
        }
        for (&name, gauge) in &other.gauges {
            self.gauges.entry(name).or_default().merge(*gauge);
        }
        for (&name, maximum) in &other.maxima {
            self.maxima.entry(name).or_default().merge(*maximum);
        }
        for (&name, timestamp) in &other.timestamps {
            self.timestamps.entry(name).or_default().merge(*timestamp);
        }
        for (&name, flag) in &other.flags {
            self.flags.entry(name).or_default().merge(*flag);
        }
        self.no_scans.merge(other.no_scans);
        self.state_severity = match (self.state_severity, other.state_severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            _ => None,
        };
        if let Some(timestamp) = other.tablespace_label_timestamp
            && let Some(label) = other.texts.get("tablespace")
        {
            self.update_tablespace_label(timestamp, label.clone());
        }
    }

    fn update_tablespace_label(&mut self, timestamp: i64, label: String) {
        let replace = self.tablespace_label_timestamp.is_none_or(|known| {
            timestamp > known
                || timestamp == known
                    && self
                        .texts
                        .get("tablespace")
                        .is_none_or(|current| label.as_bytes() < current.as_bytes())
        });
        if replace {
            self.tablespace_label_timestamp = Some(timestamp);
            self.texts.insert("tablespace", label);
        }
    }

    fn add_table(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), QueryError> {
        for &name in TABLE_RATES {
            let structural = TABLE_STRUCTURAL_RATES.contains(&name);
            self.rates
                .entry(name)
                .or_default()
                .add(counter_input(plan, row, before, name, structural), elapsed);
        }
        for &name in TABLE_GAUGES {
            let structural = TABLE_STRUCTURAL_GAUGES.contains(&name);
            let structural_na = structural && matches!(row.get("toast_bytes"), Some(Cell::Null));
            let mut input = gauge_input(plan, row, name, structural_na);
            if name == "reltuples" && matches!(input, Input::Value(OrderedNumber::Integer(-1))) {
                input = Input::Unavailable;
            }
            self.gauges.entry(name).or_default().add(input);
        }
        for &name in TABLE_MAXIMA {
            self.maxima
                .entry(name)
                .or_default()
                .add(plan.contract.column(name).is_some(), row.get(name));
        }
        for &name in TABLE_TIMESTAMPS {
            let structural_na = name == "toast_last_autovacuum"
                && matches!(row.get("toast_bytes"), Some(Cell::Null));
            let observation = if plan.contract.column(name).is_none() {
                TimestampObservation::Unavailable
            } else if structural_na {
                TimestampObservation::NotApplicable
            } else {
                TimestampObservation::Stored(row.get(name))
            };
            self.timestamps.entry(name).or_default().add(observation);
        }
        for name in ["relname", "tablespace"] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        Ok(())
    }

    fn add_index(
        &mut self,
        plan: &Plan,
        row: &Row,
        before: Option<&CounterReadings>,
        elapsed: Option<i64>,
        dictionary: &Dictionary,
    ) -> Result<(), QueryError> {
        let scan_input = counter_input(plan, row, before, "idx_scan", false);
        for &name in INDEX_RATES {
            let input = if name == "idx_scan" {
                scan_input
            } else {
                counter_input(plan, row, before, name, false)
            };
            self.rates.entry(name).or_default().add(input, elapsed);
        }
        for &name in INDEX_GAUGES {
            self.gauges
                .entry(name)
                .or_default()
                .add(gauge_input(plan, row, name, false));
        }
        for &name in INDEX_TIMESTAMPS {
            let observation = if plan.contract.column(name).is_some() {
                TimestampObservation::Stored(row.get(name))
            } else {
                TimestampObservation::Unavailable
            };
            self.timestamps.entry(name).or_default().add(observation);
        }
        for &name in INDEX_FLAGS {
            self.flags.entry(name).or_default().add(row.get(name));
        }
        self.no_scans.add_scan(scan_input);
        let valid = bool_cell(row.get("indisvalid"));
        let ready = bool_cell(row.get("indisready"));
        let severity = match (valid, ready) {
            (Some(false), _) => Some(2),
            (Some(true), Some(false)) => Some(1),
            (Some(true), Some(true)) => Some(0),
            _ => None,
        };
        self.state_severity = match (self.state_severity, severity) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, Some(value)) if self.count == 1 => Some(value),
            _ => None,
        };
        for name in [
            "indexrelname",
            "relname",
            "tablespace",
            "amname",
            "indexdef",
        ] {
            if let Some(value) = text_cell(row.get(name), dictionary)? {
                self.texts.entry(name).or_insert(value);
            }
        }
        if let Some(value) = integer_cell(row.get("relid")) {
            self.identifiers.entry("relid").or_insert(value);
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the fixed relation contract keeps every audited reducer in one exhaustive match"
    )]
    /// Calculate one public semantic metric from the accumulated physical inputs.
    #[must_use]
    pub fn metric(&self, kind: RelationKind, group: RelationGroup, name: &str) -> Option<Metric> {
        if name == kind.count_field() {
            return Some(Metric::integer(i128::from(self.count)));
        }
        if let Some(rate) = self.rates.get(name).copied() {
            return rate.metric();
        }
        if let Some(gauge) = self.gauges.get(name).copied() {
            return gauge.value().map(Metric::number);
        }
        if let Some(maximum) = self
            .maxima
            .get(name)
            .copied()
            .and_then(MaximumAggregate::exact)
        {
            return Some(Metric::integer(maximum));
        }
        if let Some(value) = self.texts.get(name) {
            return (group == RelationGroup::Object
                || group == RelationGroup::Tablespace && name == "tablespace")
                .then(|| Metric::text(value.clone()));
        }
        if let Some(value) = self.identifiers.get(name) {
            return (group == RelationGroup::Object).then_some(Metric::integer(*value));
        }
        match (kind, name) {
            (RelationKind::Tables, "sequential_share_pct") => {
                self.ratio_neutral(&["seq_scan"], &["seq_scan", "idx_scan"], 100.0)
            }
            (RelationKind::Tables, "tuple_throughput") => self
                .rate_sum_neutral(&["seq_tup_read", "idx_tup_fetch"])
                .map(Metric::rate),
            (RelationKind::Tables, "seq_tuples_per_scan") => {
                self.ratio(&["seq_tup_read"], &["seq_scan"], 1.0)
            }
            (RelationKind::Tables, "idx_tuples_per_scan")
            | (RelationKind::Indexes, "fetches_per_scan") => {
                self.ratio(&["idx_tup_fetch"], &["idx_scan"], 1.0)
            }
            (RelationKind::Tables, "last_seq_scan_never") if group == RelationGroup::Object => {
                self.never("last_seq_scan")
            }
            (RelationKind::Tables | RelationKind::Indexes, "last_idx_scan_never")
                if group == RelationGroup::Object =>
            {
                self.never("last_idx_scan")
            }
            (RelationKind::Tables, "dml_total") => self
                .rate_sum(&["n_tup_ins", "n_tup_upd", "n_tup_del"])
                .map(Metric::rate),
            (RelationKind::Tables, "insert_share_pct") => self.ratio(
                &["n_tup_ins"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "update_share_pct") => self.ratio(
                &["n_tup_upd"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "delete_share_pct") => self.ratio(
                &["n_tup_del"],
                &["n_tup_ins", "n_tup_upd", "n_tup_del"],
                100.0,
            ),
            (RelationKind::Tables, "dead_pct") => {
                self.gauge_ratio(&["n_dead_tup"], &["n_live_tup", "n_dead_tup"], 100.0)
            }
            (RelationKind::Tables, "hot_pct") => {
                self.ratio(&["n_tup_hot_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "new_page_pct") => {
                self.ratio(&["n_tup_newpage_upd"], &["n_tup_upd"], 100.0)
            }
            (RelationKind::Tables, "displayed_storage_bytes") => self
                .gauge_sum_neutral(&["main_fork_bytes", "toast_bytes"])
                .map(Metric::number),
            (RelationKind::Tables, "toast_share_pct") => self.gauge_ratio_neutral(
                &["toast_bytes"],
                &["main_fork_bytes", "toast_bytes"],
                100.0,
            ),
            (RelationKind::Tables, "toast_dead_pct") => self.gauge_ratio(
                &["toast_n_dead_tup"],
                &["toast_n_live_tup", "toast_n_dead_tup"],
                100.0,
            ),
            (RelationKind::Tables, "heap_buffer_hit_pct") => self.ratio(
                &["heap_blks_hit"],
                &["heap_blks_read", "heap_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "index_buffer_hit_pct")
            | (RelationKind::Indexes, "buffer_hit_pct") => {
                self.ratio(&["idx_blks_hit"], &["idx_blks_read", "idx_blks_hit"], 100.0)
            }
            (RelationKind::Tables, "toast_buffer_hit_pct") => self.ratio(
                &["toast_blks_hit"],
                &["toast_blks_read", "toast_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "tidx_buffer_hit_pct") => self.ratio(
                &["tidx_blks_hit"],
                &["tidx_blks_read", "tidx_blks_hit"],
                100.0,
            ),
            (RelationKind::Tables, "buffer_hit_pct") => self.ratio_neutral(
                &[
                    "heap_blks_hit",
                    "idx_blks_hit",
                    "toast_blks_hit",
                    "tidx_blks_hit",
                ],
                &[
                    "heap_blks_read",
                    "heap_blks_hit",
                    "idx_blks_read",
                    "idx_blks_hit",
                    "toast_blks_read",
                    "toast_blks_hit",
                    "tidx_blks_read",
                    "tidx_blks_hit",
                ],
                100.0,
            ),
            (RelationKind::Tables, "vacuum_mean_ms") => {
                self.ratio(&["total_vacuum_time"], &["vacuum_count"], 1.0)
            }
            (RelationKind::Tables, "autovacuum_mean_ms") => {
                self.ratio(&["total_autovacuum_time"], &["autovacuum_count"], 1.0)
            }
            (RelationKind::Tables, "analyze_mean_ms") => {
                self.ratio(&["total_analyze_time"], &["analyze_count"], 1.0)
            }
            (RelationKind::Tables, "autoanalyze_mean_ms") => {
                self.ratio(&["total_autoanalyze_time"], &["autoanalyze_count"], 1.0)
            }
            (RelationKind::Indexes, "tuples_per_scan") => {
                self.ratio(&["idx_tup_read"], &["idx_scan"], 1.0)
            }
            (RelationKind::Indexes, "no_scans") if group == RelationGroup::Object => self
                .no_scans
                .exact()
                .then_some(Metric::boolean(self.no_scans.truthy == 1)),
            (RelationKind::Indexes, "no_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::integer(i128::from(self.no_scans.truthy))),
            (RelationKind::Indexes, "known_scan_count") => self
                .no_scans
                .exact()
                .then(|| Metric::integer(i128::from(self.no_scans.known - self.no_scans.truthy))),
            (RelationKind::Indexes, "state_severity") => self.state_severity.map(Metric::integer),
            (RelationKind::Indexes, "invalid_count") => self.flag_count("indisvalid", false),
            (RelationKind::Indexes, "unready_count") => self.flag_count("indisready", false),
            (RelationKind::Indexes, "unique_count") => self.flag_count("indisunique", true),
            (RelationKind::Indexes, "primary_count") => self.flag_count("indisprimary", true),
            (RelationKind::Indexes, "exclusion_count") => self.flag_count("indisexclusion", true),
            (RelationKind::Indexes, name) if INDEX_FLAGS.contains(&name) => self
                .flags
                .get(name)
                .copied()
                .filter(|flag| flag.exact() && flag.known == 1)
                .map(|flag| Metric::boolean(flag.truthy == 1)),
            _ => timestamp_metric(self, group, name),
        }
    }

    fn ratio(&self, numerator: &[&str], denominator: &[&str], scale: f64) -> Option<Metric> {
        Some(Metric::rate_ratio(
            self.rate_sum(numerator)?,
            self.rate_sum(denominator)?,
            scale,
        ))
    }

    fn ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::rate_ratio(
            self.rate_sum_neutral(numerator)?,
            self.rate_sum_neutral(denominator)?,
            scale,
        ))
    }

    fn gauge_ratio(&self, numerator: &[&str], denominator: &[&str], scale: f64) -> Option<Metric> {
        Some(Metric::ratio(
            sum_values(&self.gauges, numerator)?,
            sum_values(&self.gauges, denominator)?,
            scale,
        ))
    }

    fn gauge_ratio_neutral(
        &self,
        numerator: &[&str],
        denominator: &[&str],
        scale: f64,
    ) -> Option<Metric> {
        Some(Metric::ratio(
            self.gauge_sum_neutral(numerator)?,
            self.gauge_sum_neutral(denominator)?,
            scale,
        ))
    }

    fn rate_sum(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates(&self.rates, names)
    }

    fn rate_sum_neutral(&self, names: &[&str]) -> Option<RateValue> {
        sum_rates_neutral(&self.rates, names)
    }

    fn gauge_sum_neutral(&self, names: &[&str]) -> Option<OrderedNumber> {
        sum_values_neutral(&self.gauges, names)
    }

    fn never(&self, name: &str) -> Option<Metric> {
        let timestamp = self.timestamps.get(name).copied()?;
        timestamp
            .exact()
            .then_some(Metric::boolean(timestamp.never == 1))
    }

    fn flag_count(&self, name: &str, truthy: bool) -> Option<Metric> {
        let flag = self.flags.get(name).copied()?;
        flag.exact().then(|| {
            let count = if truthy {
                flag.truthy
            } else {
                flag.known - flag.truthy
            };
            Metric::integer(i128::from(count))
        })
    }
}

fn timestamp_metric(
    aggregate: &RelationAggregate,
    group: RelationGroup,
    name: &str,
) -> Option<Metric> {
    if group == RelationGroup::Object {
        let timestamp = aggregate.timestamps.get(name).copied()?;
        return (timestamp.exact() && timestamp.never == 0)
            .then(|| timestamp.latest.map(Metric::timestamp))
            .flatten();
    }
    for suffix in ["_oldest", "_latest", "_never_count"] {
        let Some(base) = name.strip_suffix(suffix) else {
            continue;
        };
        let timestamp = aggregate.timestamps.get(base).copied()?;
        if !timestamp.exact() {
            return None;
        }
        return match suffix {
            "_oldest" => timestamp.oldest.map(Metric::timestamp),
            "_latest" => timestamp.latest.map(Metric::timestamp),
            "_never_count" => Some(Metric::integer(i128::from(timestamp.never))),
            _ => None,
        };
    }
    None
}

fn sum_rates(rates: &BTreeMap<&'static str, RateAggregate>, names: &[&str]) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let value = rates.get(name)?.value()?;
        sum = Some(match sum {
            Some(known) => known.add(value)?,
            None => value,
        });
    }
    sum
}

fn sum_rates_neutral(
    rates: &BTreeMap<&'static str, RateAggregate>,
    names: &[&str],
) -> Option<RateValue> {
    let mut sum: Option<RateValue> = None;
    for name in names {
        let rate = rates.get(name)?;
        if rate.unavailable {
            return None;
        }
        let Some(value) = rate.value else {
            continue;
        };
        sum = Some(match sum {
            Some(known) => known.add(value)?,
            None => value,
        });
    }
    sum
}

fn sum_values(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        add_ordered(sum, values.get(name)?.value()?).map(Some)
    })?
}

fn sum_values_neutral(
    values: &BTreeMap<&'static str, Availability>,
    names: &[&str],
) -> Option<OrderedNumber> {
    names.iter().try_fold(None, |sum, name| {
        let value = match values.get(name)? {
            Availability::Value(value) => *value,
            Availability::Empty => OrderedNumber::Integer(0),
            Availability::Unavailable => return None,
        };
        add_ordered(sum, value).map(Some)
    })?
}

/// Opaque relation metric shared with transport adapters.
#[derive(Clone)]
pub struct Metric {
    value: MetricValue,
}

impl std::fmt::Debug for Metric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Metric").field(&self.json()).finish()
    }
}

#[derive(Clone)]
enum MetricValue {
    Number(OrderedNumber),
    Rate(RateValue),
    RateRatio {
        numerator: RateValue,
        denominator: RateValue,
        scale: f64,
    },
    Ratio {
        numerator: OrderedNumber,
        denominator: OrderedNumber,
        scale: f64,
    },
    Integer(i128),
    Timestamp(i64),
    Boolean(bool),
    Text(String),
}

impl Metric {
    const fn number(value: OrderedNumber) -> Self {
        Self {
            value: MetricValue::Number(value),
        }
    }

    const fn rate(value: RateValue) -> Self {
        Self {
            value: MetricValue::Rate(value),
        }
    }

    const fn rate_ratio(numerator: RateValue, denominator: RateValue, scale: f64) -> Self {
        Self {
            value: MetricValue::RateRatio {
                numerator,
                denominator,
                scale,
            },
        }
    }

    const fn ratio(numerator: OrderedNumber, denominator: OrderedNumber, scale: f64) -> Self {
        Self {
            value: MetricValue::Ratio {
                numerator,
                denominator,
                scale,
            },
        }
    }

    const fn integer(value: i128) -> Self {
        Self {
            value: MetricValue::Integer(value),
        }
    }

    const fn timestamp(value: i64) -> Self {
        Self {
            value: MetricValue::Timestamp(value),
        }
    }

    const fn boolean(value: bool) -> Self {
        Self {
            value: MetricValue::Boolean(value),
        }
    }

    const fn text(value: String) -> Self {
        Self {
            value: MetricValue::Text(value),
        }
    }

    /// Render this metric using the existing JSON scalar contract.
    #[must_use]
    pub fn json(&self) -> Value {
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Value::String(value.to_string())
            }
            MetricValue::Number(OrderedNumber::Float(value)) => finite_json(*value),
            MetricValue::Rate(value) => finite_json(value.per_second()),
            MetricValue::RateRatio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.per_microsecond() / denominator.per_microsecond() * scale),
            MetricValue::Ratio {
                numerator,
                denominator,
                scale,
            } => finite_json(numerator.as_f64() / denominator.as_f64() * scale),
            MetricValue::Timestamp(value) => Value::String(value.to_string()),
            MetricValue::Boolean(value) => Value::Bool(*value),
            MetricValue::Text(value) => Value::String(value.clone()),
        }
    }

    /// Compare two compatible metric values without applying transport sort direction.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        compare_metric_order(self.order_value()?, other.order_value()?)
    }

    /// Compare this metric with an exact non-negative rational threshold.
    #[must_use]
    pub fn compare_ratio(&self, numerator: u128, denominator: u128) -> Option<Ordering> {
        if denominator == 0 {
            return None;
        }
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Some(compare_u128_ratios(
                    u128::try_from(*value).ok()?,
                    1,
                    numerator,
                    denominator,
                ))
            }
            MetricValue::Number(OrderedNumber::Float(value)) => {
                compare_float_ratio(*value, numerator, denominator)
            }
            MetricValue::Rate(RateValue::Exact {
                numerator: value,
                denominator: value_denominator,
            }) => Some(compare_products(
                &[*value, 1_000_000, denominator],
                &[*value_denominator, numerator],
            )),
            MetricValue::Rate(RateValue::Float(value)) => {
                compare_float_ratio(*value * 1_000_000.0, numerator, denominator)
            }
            MetricValue::RateRatio {
                numerator: value_numerator,
                denominator: value_denominator,
                scale,
            } => match (*value_numerator, *value_denominator) {
                (
                    RateValue::Exact {
                        numerator: value,
                        denominator: numerator_denominator,
                    },
                    RateValue::Exact {
                        numerator: denominator_numerator,
                        denominator: denominator_denominator,
                    },
                ) if denominator_numerator > 0 => Some(compare_products(
                    &[
                        value,
                        denominator_denominator,
                        exact_scale(*scale)?,
                        denominator,
                    ],
                    &[numerator_denominator, denominator_numerator, numerator],
                )),
                _ => compare_float_ratio(
                    value_numerator.per_microsecond() / value_denominator.per_microsecond() * scale,
                    numerator,
                    denominator,
                ),
            },
            MetricValue::Ratio {
                numerator: value_numerator,
                denominator: value_denominator,
                scale,
            } => match (*value_numerator, *value_denominator) {
                (OrderedNumber::Integer(value), OrderedNumber::Integer(value_denominator))
                    if value >= 0 && value_denominator > 0 =>
                {
                    Some(compare_products(
                        &[
                            u128::try_from(value).ok()?,
                            exact_scale(*scale)?,
                            denominator,
                        ],
                        &[u128::try_from(value_denominator).ok()?, numerator],
                    ))
                }
                _ => compare_float_ratio(
                    value_numerator.as_f64() / value_denominator.as_f64() * scale,
                    numerator,
                    denominator,
                ),
            },
            MetricValue::Timestamp(_) | MetricValue::Boolean(_) | MetricValue::Text(_) => None,
        }
    }

    fn order_value(&self) -> Option<MetricOrderValue<'_>> {
        match &self.value {
            MetricValue::Number(OrderedNumber::Integer(value)) | MetricValue::Integer(value) => {
                Some(MetricOrderValue::Integer(*value))
            }
            MetricValue::Number(OrderedNumber::Float(value)) => {
                value.is_finite().then_some(MetricOrderValue::Float(*value))
            }
            MetricValue::Rate(value) => rate_order_value(*value),
            MetricValue::RateRatio {
                numerator,
                denominator,
                ..
            } => rate_ratio_order_value(*numerator, *denominator),
            MetricValue::Ratio {
                numerator: OrderedNumber::Integer(numerator),
                denominator: OrderedNumber::Integer(denominator),
                ..
            } if *numerator >= 0 && *denominator > 0 => Some(MetricOrderValue::ExactRatio {
                numerator: u128::try_from(*numerator).ok()?,
                denominator: u128::try_from(*denominator).ok()?,
            }),
            MetricValue::Ratio {
                numerator,
                denominator,
                ..
            } => {
                let denominator = denominator.as_f64();
                let ratio = numerator.as_f64() / denominator;
                (denominator > 0.0 && ratio.is_finite())
                    .then_some(MetricOrderValue::FloatRatio(ratio))
            }
            MetricValue::Timestamp(value) => Some(MetricOrderValue::Integer(i128::from(*value))),
            MetricValue::Boolean(value) => Some(MetricOrderValue::Integer(i128::from(*value))),
            MetricValue::Text(value) => Some(MetricOrderValue::Text(value.as_bytes())),
        }
    }
}

#[derive(Clone, Copy)]
enum MetricOrderValue<'a> {
    Integer(i128),
    Float(f64),
    ExactRatio { numerator: u128, denominator: u128 },
    FloatRatio(f64),
    Text(&'a [u8]),
}

fn rate_order_value(value: RateValue) -> Option<MetricOrderValue<'static>> {
    match value {
        RateValue::Exact {
            numerator,
            denominator,
        } => Some(MetricOrderValue::ExactRatio {
            numerator,
            denominator,
        }),
        RateValue::Float(value) => value
            .is_finite()
            .then_some(MetricOrderValue::FloatRatio(value)),
    }
}

fn rate_ratio_order_value(
    numerator: RateValue,
    denominator: RateValue,
) -> Option<MetricOrderValue<'static>> {
    if let (
        RateValue::Exact {
            numerator,
            denominator: numerator_denominator,
        },
        RateValue::Exact {
            numerator: denominator_numerator,
            denominator: denominator_denominator,
        },
    ) = (numerator, denominator)
        && denominator_numerator > 0
        && let Some((numerator, denominator)) = numerator
            .checked_mul(denominator_denominator)
            .zip(numerator_denominator.checked_mul(denominator_numerator))
    {
        let RateValue::Exact {
            numerator,
            denominator,
        } = RateValue::exact(numerator, denominator)
        else {
            unreachable!("an exact rate remains exact")
        };
        return Some(MetricOrderValue::ExactRatio {
            numerator,
            denominator,
        });
    }
    let denominator_value = denominator.per_microsecond();
    let value = numerator.per_microsecond() / denominator_value;
    (denominator_value > 0.0 && value.is_finite()).then_some(MetricOrderValue::FloatRatio(value))
}

fn compare_metric_order(
    left: MetricOrderValue<'_>,
    right: MetricOrderValue<'_>,
) -> Option<Ordering> {
    match (left, right) {
        (MetricOrderValue::Integer(left), MetricOrderValue::Integer(right)) => {
            Some(left.cmp(&right))
        }
        (MetricOrderValue::Float(left), MetricOrderValue::Float(right))
        | (MetricOrderValue::FloatRatio(left), MetricOrderValue::FloatRatio(right)) => {
            left.partial_cmp(&right)
        }
        (
            MetricOrderValue::ExactRatio {
                numerator: left_numerator,
                denominator: left_denominator,
            },
            MetricOrderValue::ExactRatio {
                numerator: right_numerator,
                denominator: right_denominator,
            },
        ) => Some(compare_u128_ratios(
            left_numerator,
            left_denominator,
            right_numerator,
            right_denominator,
        )),
        (
            MetricOrderValue::ExactRatio {
                numerator,
                denominator,
            },
            MetricOrderValue::FloatRatio(right),
        ) => integer_ratio_as_f64(numerator, denominator).partial_cmp(&right),
        (
            MetricOrderValue::FloatRatio(left),
            MetricOrderValue::ExactRatio {
                numerator,
                denominator,
            },
        ) => left.partial_cmp(&integer_ratio_as_f64(numerator, denominator)),
        (MetricOrderValue::Text(left), MetricOrderValue::Text(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "this path compares an exact ratio with an already inexact floating source"
)]
fn integer_ratio_as_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "floating comparison is used only when the authoritative reducer is already inexact"
)]
fn compare_float_ratio(value: f64, numerator: u128, denominator: u128) -> Option<Ordering> {
    let threshold = numerator as f64 / denominator as f64;
    (value.is_finite() && threshold.is_finite()).then(|| value.total_cmp(&threshold))
}

const fn exact_scale(scale: f64) -> Option<u128> {
    match scale.to_bits() {
        bits if bits == 1.0_f64.to_bits() => Some(1),
        bits if bits == 100.0_f64.to_bits() => Some(100),
        _ => None,
    }
}

fn compare_u128_ratios(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reverse = false;
    loop {
        let whole = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if !matches!(whole, Ordering::Equal) {
            return if reverse { whole.reverse() } else { whole };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => {
                return if reverse {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (false, true) => {
                return if reverse {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (false, false) => {}
        }
        left_numerator = left_denominator;
        left_denominator = left_remainder;
        right_numerator = right_denominator;
        right_denominator = right_remainder;
        reverse = !reverse;
    }
}

/// Whether one physical index row has an exact zero scan rate.
#[must_use]
pub fn index_scan_rate_is_zero(
    plan: &Plan,
    row: &Row,
    before: Option<&BTreeMap<&'static str, Cell>>,
    elapsed: Option<i64>,
) -> bool {
    let mut scans = RateAggregate::default();
    scans.add(counter_input(plan, row, before, "idx_scan", false), elapsed);
    scans.metric().is_some_and(|metric| match metric.value {
        MetricValue::Rate(RateValue::Exact { numerator, .. }) => numerator == 0,
        MetricValue::Rate(RateValue::Float(value)) => value == 0.0,
        _ => false,
    })
}

fn finite_json(value: f64) -> Value {
    if value.is_finite() {
        json!(value)
    } else {
        Value::Null
    }
}

struct HistorySegment {
    segment: DatasetSegment,
    plans: Vec<Plan>,
}

struct HistoryPrevious {
    timestamp: i64,
    readings: CounterReadings,
}

#[derive(Clone, Copy)]
struct HistoryMoment {
    type_id: u32,
    previous: Option<i64>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one grouped stream keeps validation, the fixed two passes, and ordered emission together"
)]
pub(super) fn stream_history(
    dataset: &dyn QueryDataset,
    listed: &[DatasetSegment],
    window: Window,
    request: &HourSeriesRequest,
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let group = request
        .group
        .filter(|group| *group != RelationGroup::Object)
        .ok_or_else(|| QueryError::BadFilter("group".to_owned()))?;
    let kind = RelationKind::from_name(&request.section)?;
    let fields = output_fields(
        std::slice::from_ref(&request.section),
        group,
        &request.fields,
    )?;
    if fields.is_empty() || request.type_id.is_some() {
        return Err(QueryError::BadFilter("group".to_owned()));
    }
    if group == RelationGroup::Tablespace {
        return stream_tablespace_history(dataset, listed, window, request, kind, &fields, sink);
    }
    let datid = history_datid(group, &request.filters)?;
    let Some((from, to)) = window.from.zip(window.to) else {
        return Ok(());
    };
    let refs = history_segments(dataset, listed, &request.section, datid, from, to, sink)?;
    let physical_fields = physical_fields(kind, group, &fields);
    let mut sources = Vec::with_capacity(refs.len());
    for segment_ref in refs {
        if sink.cancelled() {
            return Ok(());
        }
        let segment = dataset.open(&segment_ref)?;
        let fields = physical_fields
            .iter()
            .filter(|name| {
                segment
                    .layouts(&request.section)
                    .filter_map(|(type_id, _section)| contract(type_id))
                    .any(|layout| layout.column(name).is_some())
            })
            .cloned()
            .collect();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: segment_ref.id(),
                section: request.section.clone(),
            },
            fields,
            filters: request.filters.clone(),
            type_id: None,
            after: None,
        };
        match plans(&segment, &data, true) {
            Ok(plans) => sources.push(HistorySegment {
                segment: segment_ref,
                plans,
            }),
            Err(QueryError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    if sink.cancelled() || !sink.record(relation_layout(&request.section, kind, group, &fields)?) {
        return Ok(());
    }
    let selected = selected_history_layouts(dataset, &sources, datid, from, to, sink)?;
    if sink.cancelled() {
        return Ok(());
    }
    let mut previous = BTreeMap::<(u32, Vec<IdentityCell>), HistoryPrevious>::new();
    let mut aggregates = BTreeMap::<(i64, GroupKey), RelationAggregate>::new();
    for source in &sources {
        if sink.cancelled() {
            return Ok(());
        }
        let segment = dataset.open(&source.segment)?;
        for plan in &source.plans {
            scan_history_plan(
                &segment,
                plan,
                kind,
                group,
                datid,
                from,
                to,
                &selected,
                &mut previous,
                &mut aggregates,
                sink,
            )?;
            if sink.cancelled() {
                return Ok(());
            }
        }
    }
    let mut segment_id = None;
    for ((_timestamp, _key), aggregate) in aggregates {
        if segment_id != Some(aggregate.source.segment_id) {
            segment_id = Some(aggregate.source.segment_id);
            if !sink.record(record(json!({
                "record": "series_segment",
                "segment": { "id": aggregate.source.segment_id.to_string() },
            }))?) {
                return Ok(());
            }
        }
        let metrics = fields
            .iter()
            .map(|name| (name.clone(), aggregate.metric(kind, group, name)))
            .collect();
        let row = RelationRow {
            key: aggregate.key,
            metrics,
            from: aggregate.from,
            to: aggregate.to,
        };
        if sink.cancelled() || !sink.record(relation_record(&request.section, kind, group, &row)?) {
            return Ok(());
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "the cross-database as-of reducer keeps scan and emission state adjacent"
)]
fn stream_tablespace_history(
    dataset: &dyn QueryDataset,
    listed: &[DatasetSegment],
    window: Window,
    request: &HourSeriesRequest,
    kind: RelationKind,
    fields: &[String],
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let tablespace_oid = history_tablespace_oid(&request.filters)?;
    if sink.cancelled()
        || !sink.record(relation_layout(
            &request.section,
            kind,
            RelationGroup::Tablespace,
            fields,
        )?)
    {
        return Ok(());
    }
    let Some((from, to)) = window.from.zip(window.to) else {
        return Ok(());
    };
    let (refs, datids) =
        tablespace_history_segments(dataset, listed, &request.section, from, to, sink)?;
    if datids.is_empty() || sink.cancelled() {
        return Ok(());
    }
    let physical_fields = physical_fields(kind, RelationGroup::Tablespace, fields);
    let mut sources = Vec::with_capacity(refs.len());
    for segment_ref in refs {
        if sink.cancelled() {
            return Ok(());
        }
        let segment = dataset.open(&segment_ref)?;
        let projected = physical_fields
            .iter()
            .filter(|name| {
                segment
                    .layouts(&request.section)
                    .filter_map(|(type_id, _section)| contract(type_id))
                    .any(|layout| layout.column(name).is_some())
            })
            .cloned()
            .collect();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: segment_ref.id(),
                section: request.section.clone(),
            },
            fields: projected,
            filters: Vec::new(),
            type_id: None,
            after: None,
        };
        match plans(&segment, &data, true) {
            Ok(plans) => sources.push(HistorySegment {
                segment: segment_ref,
                plans,
            }),
            Err(QueryError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    let selected = selected_tablespace_history_layouts(dataset, &sources, &datids, from, to, sink)?;
    let mut previous = BTreeMap::<(u32, Vec<IdentityCell>), HistoryPrevious>::new();
    let mut contributions = BTreeMap::<(i64, u32), RelationAggregate>::new();
    let mut event_sources = BTreeMap::<(i64, u32), RelationSource>::new();
    for source in &sources {
        if sink.cancelled() {
            return Ok(());
        }
        let segment = dataset.open(&source.segment)?;
        for plan in &source.plans {
            scan_tablespace_history_plan(
                &segment,
                plan,
                kind,
                tablespace_oid,
                to,
                &datids,
                &selected,
                &mut previous,
                &mut contributions,
                &mut event_sources,
                sink,
            )?;
            if sink.cancelled() {
                return Ok(());
            }
        }
    }
    let mut current = BTreeMap::<u32, RelationAggregate>::new();
    let events = selected.keys().copied().collect::<Vec<_>>();
    let mut cursor = 0;
    let mut emitted_segment = None;
    while cursor < events.len() {
        let timestamp = events[cursor].0;
        let mut event_source = None;
        while cursor < events.len() && events[cursor].0 == timestamp {
            let event = events[cursor];
            if let Some(aggregate) = contributions.remove(&event) {
                current.insert(event.1, aggregate);
            } else {
                current.remove(&event.1);
            }
            if let Some(source) = event_sources.get(&event).copied() {
                event_source =
                    Some(event_source.map_or(source, |known: RelationSource| known.min(source)));
            }
            cursor += 1;
        }
        if timestamp < from || timestamp > to {
            continue;
        }
        let Some(mut aggregate) = current.values().next().cloned() else {
            continue;
        };
        for member in current.values().skip(1) {
            aggregate.merge(member);
        }
        let Some(mut source) = event_source else {
            continue;
        };
        source.timestamp = timestamp;
        aggregate.source = source;
        aggregate.to = Some(timestamp);
        if emitted_segment != Some(source.segment_id) {
            emitted_segment = Some(source.segment_id);
            if !sink.record(record(json!({
                "record": "series_segment",
                "segment": { "id": source.segment_id.to_string() },
            }))?) {
                return Ok(());
            }
        }
        let metrics = fields
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    aggregate.metric(kind, RelationGroup::Tablespace, name),
                )
            })
            .collect();
        let row = RelationRow {
            key: aggregate.key,
            metrics,
            from: aggregate.from,
            to: aggregate.to,
        };
        if sink.cancelled()
            || !sink.record(relation_record(
                &request.section,
                kind,
                RelationGroup::Tablespace,
                &row,
            )?)
        {
            return Ok(());
        }
    }
    Ok(())
}

fn history_tablespace_oid(filters: &[Filter]) -> Result<u32, QueryError> {
    if filters.len() != 1 || filters[0].column != "tablespace_oid" {
        return Err(QueryError::BadFilter("where".to_owned()));
    }
    filters[0]
        .value
        .parse::<u32>()
        .ok()
        .filter(|oid| *oid != 0)
        .ok_or_else(|| QueryError::BadFilter("tablespace_oid".to_owned()))
}

fn tablespace_history_segments(
    dataset: &dyn QueryDataset,
    listed: &[DatasetSegment],
    logical_name: &str,
    from: i64,
    to: i64,
    sink: &dyn QuerySink,
) -> Result<(Vec<DatasetSegment>, HashSet<u32>), QueryError> {
    let has_section = |segment: &DatasetSegment| {
        segment
            .sections()
            .iter()
            .any(|section| logical_section_name(section.type_id) == Some(logical_name))
    };
    let mut selected = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.max_ts() >= from && segment.min_ts() <= to
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut predecessors = BTreeMap::<(u32, u32), Vec<i64>>::new();
    let mut required = HashSet::new();
    for segment_ref in &selected {
        collect_tablespace_moments(
            dataset,
            segment_ref,
            logical_name,
            from,
            to,
            true,
            &mut required,
            &mut predecessors,
            sink,
        )?;
    }
    predecessors.retain(|key, _moments| required.contains(key));
    for key in &required {
        predecessors.entry(*key).or_default();
    }
    let selected_ids = selected
        .iter()
        .map(DatasetSegment::id)
        .collect::<HashSet<_>>();
    let mut candidates = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.min_ts() < from && !selected_ids.contains(&segment.id())
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|segment| (segment.max_ts(), segment.id()));
    for candidate in candidates.into_iter().rev() {
        if sink.cancelled()
            || required.iter().all(|pair| {
                predecessors
                    .get(pair)
                    .is_some_and(|moments| moments.len() >= 2)
            })
        {
            break;
        }
        let changed = collect_tablespace_moments(
            dataset,
            candidate,
            logical_name,
            from,
            to,
            false,
            &mut required,
            &mut predecessors,
            sink,
        )?;
        if changed {
            selected.push((*candidate).clone());
        }
    }
    selected.sort_unstable_by_key(DatasetSegment::id);
    selected.dedup_by_key(|segment| segment.id());
    let datids = required
        .into_iter()
        .map(|(datid, _type_id)| datid)
        .collect();
    Ok((selected, datids))
}

#[expect(
    clippy::too_many_arguments,
    reason = "moment discovery keeps its exact time bounds and predecessor state explicit"
)]
fn collect_tablespace_moments(
    dataset: &dyn QueryDataset,
    segment_ref: &DatasetSegment,
    logical_name: &str,
    from: i64,
    to: i64,
    discover: bool,
    required: &mut HashSet<(u32, u32)>,
    moments: &mut BTreeMap<(u32, u32), Vec<i64>>,
    sink: &dyn QuerySink,
) -> Result<bool, QueryError> {
    let segment = dataset.open(segment_ref)?;
    let mut changed = false;
    for (type_id, _section) in segment.layouts(logical_name) {
        let Some(timestamp) = contract(type_id).and_then(|layout| {
            layout
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
        }) else {
            continue;
        };
        segment.visit_rows(
            type_id,
            &[timestamp, "datid"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if sink.cancelled() {
                    return false;
                }
                let (Some(datid), Some(stored)) = (
                    unsigned_cell(row.get("datid")),
                    timestamp_cell(row.get(timestamp)),
                ) else {
                    return true;
                };
                let key = (datid, type_id);
                if discover && (from..=to).contains(&stored) {
                    required.insert(key);
                }
                if stored < from {
                    let known = if discover {
                        Some(moments.entry(key).or_default())
                    } else {
                        moments.get_mut(&key)
                    };
                    if let Some(known) = known
                        && !known.contains(&stored)
                    {
                        known.push(stored);
                        known.sort_unstable_by(|left, right| right.cmp(left));
                        known.truncate(2);
                        changed = true;
                    }
                }
                true
            },
        )?;
    }
    Ok(changed)
}

fn selected_tablespace_history_layouts(
    dataset: &dyn QueryDataset,
    sources: &[HistorySegment],
    datids: &HashSet<u32>,
    from: i64,
    to: i64,
    sink: &dyn QuerySink,
) -> Result<BTreeMap<(i64, u32), HistoryMoment>, QueryError> {
    let mut by_layout = BTreeMap::<(u32, u32), std::collections::BTreeSet<i64>>::new();
    for source in sources {
        if sink.cancelled() {
            break;
        }
        let segment = dataset.open(&source.segment)?;
        for plan in &source.plans {
            let Some(timestamp) = plan.timestamp else {
                continue;
            };
            segment.visit_rows(
                plan.type_id,
                &[timestamp, "datid"],
                0,
                usize::MAX,
                |_ordinal, row| {
                    let (Some(datid), Some(stored)) = (
                        unsigned_cell(row.get("datid")),
                        timestamp_cell(row.get(timestamp)),
                    ) else {
                        return !sink.cancelled();
                    };
                    if datids.contains(&datid) && stored <= to {
                        by_layout
                            .entry((datid, plan.type_id))
                            .or_default()
                            .insert(stored);
                    }
                    !sink.cancelled()
                },
            )?;
        }
    }
    let mut selected = BTreeMap::<(i64, u32), HistoryMoment>::new();
    let mut seeds = BTreeMap::<u32, (i64, HistoryMoment)>::new();
    for ((datid, type_id), moments) in &by_layout {
        let mut previous = None;
        for timestamp in moments {
            let candidate = HistoryMoment {
                type_id: *type_id,
                previous,
            };
            if (from..=to).contains(timestamp) {
                selected
                    .entry((*timestamp, *datid))
                    .and_modify(|chosen| {
                        if candidate.type_id > chosen.type_id {
                            *chosen = candidate;
                        }
                    })
                    .or_insert(candidate);
            } else if *timestamp < from {
                seeds
                    .entry(*datid)
                    .and_modify(|chosen| {
                        if *timestamp > chosen.0
                            || *timestamp == chosen.0 && candidate.type_id > chosen.1.type_id
                        {
                            *chosen = (*timestamp, candidate);
                        }
                    })
                    .or_insert((*timestamp, candidate));
            }
            previous = Some(*timestamp);
        }
    }
    for (datid, (timestamp, moment)) in seeds {
        selected.insert((timestamp, datid), moment);
    }
    Ok(selected)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one scan keeps exact object predecessors, event membership, and source coordinates"
)]
fn scan_tablespace_history_plan(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    tablespace_oid: u32,
    to: i64,
    datids: &HashSet<u32>,
    selected: &BTreeMap<(i64, u32), HistoryMoment>,
    previous: &mut BTreeMap<(u32, Vec<IdentityCell>), HistoryPrevious>,
    contributions: &mut BTreeMap<(i64, u32), RelationAggregate>,
    event_sources: &mut BTreeMap<(i64, u32), RelationSource>,
    sink: &dyn QuerySink,
) -> Result<(), QueryError> {
    if plan.timestamp.is_none() {
        return Ok(());
    }
    let counters = rate_columns(plan);
    let mut chunk = Vec::with_capacity(HISTORY_CHUNK_ROWS);
    let mut failure = None;
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        0,
        usize::MAX,
        |ordinal, row| {
            chunk.push((ordinal, row));
            if chunk.len() == HISTORY_CHUNK_ROWS
                && let Err(error) = process_tablespace_history_chunk(
                    segment,
                    plan,
                    kind,
                    tablespace_oid,
                    to,
                    datids,
                    selected,
                    &counters,
                    previous,
                    contributions,
                    event_sources,
                    &mut chunk,
                )
            {
                failure = Some(error);
                return false;
            }
            !sink.cancelled()
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !sink.cancelled() && !chunk.is_empty() {
        process_tablespace_history_chunk(
            segment,
            plan,
            kind,
            tablespace_oid,
            to,
            datids,
            selected,
            &counters,
            previous,
            contributions,
            event_sources,
            &mut chunk,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bounded chunk shares exact predecessor and per-database aggregate state"
)]
fn process_tablespace_history_chunk(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    tablespace_oid: u32,
    to: i64,
    datids: &HashSet<u32>,
    selected: &BTreeMap<(i64, u32), HistoryMoment>,
    counters: &[&'static str],
    previous: &mut BTreeMap<(u32, Vec<IdentityCell>), HistoryPrevious>,
    contributions: &mut BTreeMap<(i64, u32), RelationAggregate>,
    event_sources: &mut BTreeMap<(i64, u32), RelationSource>,
    chunk: &mut Vec<(u64, Row)>,
) -> Result<(), QueryError> {
    let dictionary = chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        let (Some(datid), Some(timestamp), Some(identity)) = (
            unsigned_cell(row.get("datid")),
            plan.timestamp
                .and_then(|name| timestamp_cell(row.get(name))),
            identity_of(plan, &row),
        ) else {
            continue;
        };
        if !datids.contains(&datid) || timestamp > to {
            continue;
        }
        let history_key = (plan.type_id, identity);
        let before = previous.get(&history_key);
        if before.is_some_and(|stored| stored.timestamp >= timestamp) {
            continue;
        }
        let event = (timestamp, datid);
        let moment = selected.get(&event).copied();
        let required_previous = moment
            .filter(|moment| moment.type_id == plan.type_id)
            .and_then(|moment| moment.previous);
        let elapsed = required_previous
            .and_then(|stored| timestamp.checked_sub(stored))
            .filter(|elapsed| *elapsed > 0);
        let exact_before = before.filter(|stored| Some(stored.timestamp) == required_previous);
        if moment.is_some_and(|moment| moment.type_id == plan.type_id) {
            let source = RelationSource {
                segment_id: segment.id(),
                context_index: 0,
                ordinal,
                type_id: plan.type_id,
                timestamp,
            };
            event_sources
                .entry(event)
                .and_modify(|known| *known = (*known).min(source))
                .or_insert(source);
            if unsigned_cell(row.get("tablespace_oid")) == Some(tablespace_oid) {
                let key = GroupKey(GroupKeyValue::Tablespace { tablespace_oid });
                contributions
                    .entry(event)
                    .or_insert_with(|| RelationAggregate::new(key, source))
                    .add(
                        kind,
                        plan,
                        &row,
                        exact_before.map(|stored| &stored.readings),
                        elapsed,
                        &dictionary,
                        source,
                    )?;
            }
        }
        let readings = counters
            .iter()
            .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
            .collect();
        previous.insert(
            history_key,
            HistoryPrevious {
                timestamp,
                readings,
            },
        );
    }
    Ok(())
}

fn physical_fields(kind: RelationKind, group: RelationGroup, fields: &[String]) -> Vec<String> {
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

fn history_datid(group: RelationGroup, filters: &[Filter]) -> Result<u32, QueryError> {
    let required: &[&str] = match group {
        RelationGroup::Database => &["datid"],
        RelationGroup::Schema => &["datid", "schemaname"],
        RelationGroup::Tablespace | RelationGroup::Object => {
            return Err(QueryError::BadFilter("group".to_owned()));
        }
    };
    if filters.len() != required.len()
        || required
            .iter()
            .any(|name| !filters.iter().any(|filter| filter.column == *name))
    {
        return Err(QueryError::BadFilter("where".to_owned()));
    }
    filters
        .iter()
        .find(|filter| filter.column == "datid")
        .and_then(|filter| filter.value.parse().ok())
        .ok_or_else(|| QueryError::BadFilter("datid".to_owned()))
}

fn history_segments(
    dataset: &dyn QueryDataset,
    listed: &[DatasetSegment],
    logical_name: &str,
    datid: u32,
    from: i64,
    to: i64,
    sink: &dyn QuerySink,
) -> Result<Vec<DatasetSegment>, QueryError> {
    let has_section = |segment: &DatasetSegment| {
        segment
            .sections()
            .iter()
            .any(|section| logical_section_name(section.type_id) == Some(logical_name))
    };
    let mut selected = listed
        .iter()
        .filter(|segment| {
            has_section(segment) && segment.max_ts() >= from && segment.min_ts() <= to
        })
        .cloned()
        .collect::<Vec<_>>();
    let type_ids = selected
        .iter()
        .flat_map(DatasetSegment::sections)
        .filter(|section| logical_section_name(section.type_id) == Some(logical_name))
        .map(|section| section.type_id)
        .collect::<HashSet<_>>();
    let selected_ids = selected
        .iter()
        .map(DatasetSegment::id)
        .collect::<HashSet<_>>();
    let mut candidates = listed
        .iter()
        .filter(|segment| segment.min_ts() < from && !selected_ids.contains(&segment.id()))
        .filter(|segment| {
            segment
                .sections()
                .iter()
                .any(|section| type_ids.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|segment| (segment.max_ts(), segment.id()));
    let mut predecessors = BTreeMap::<u32, (i64, Vec<DatasetSegment>)>::new();
    for candidate in candidates.into_iter().rev() {
        if sink.cancelled() {
            break;
        }
        if type_ids.iter().all(|type_id| {
            predecessors
                .get(type_id)
                .is_some_and(|(timestamp, _segments)| candidate.max_ts() < *timestamp)
        }) {
            break;
        }
        let segment = dataset.open(candidate)?;
        let carried = candidate
            .sections()
            .iter()
            .map(|section| section.type_id)
            .filter(|type_id| type_ids.contains(type_id))
            .collect::<Vec<_>>();
        for type_id in carried {
            let Some(timestamp) = segment_datid_predecessor(&segment, type_id, datid, from, sink)?
            else {
                continue;
            };
            match predecessors.entry(type_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((timestamp, vec![(*candidate).clone()]));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let (chosen, segments) = entry.get_mut();
                    match timestamp.cmp(chosen) {
                        Ordering::Greater => {
                            *chosen = timestamp;
                            segments.clear();
                            segments.push((*candidate).clone());
                        }
                        Ordering::Equal => segments.push((*candidate).clone()),
                        Ordering::Less => {}
                    }
                }
            }
        }
    }
    selected.extend(
        predecessors
            .into_values()
            .flat_map(|(_timestamp, segments)| segments),
    );
    selected.sort_unstable_by_key(DatasetSegment::id);
    selected.dedup_by_key(|segment| segment.id());
    Ok(selected)
}

fn segment_datid_predecessor(
    segment: &Segment,
    type_id: u32,
    datid: u32,
    before: i64,
    sink: &dyn QuerySink,
) -> Result<Option<i64>, QueryError> {
    if segment.rows_of(type_id).is_none() {
        return Ok(None);
    }
    let Some(timestamp) = contract(type_id).and_then(|layout| {
        layout
            .columns
            .iter()
            .find(|column| column.class == ColumnClass::Timestamp)
            .map(|column| column.name)
    }) else {
        return Ok(None);
    };
    let mut found = None;
    segment.visit_rows(
        type_id,
        &[timestamp, "datid"],
        0,
        usize::MAX,
        |_ordinal, row| {
            if unsigned_cell(row.get("datid")) == Some(datid)
                && let Some(stored) = timestamp_cell(row.get(timestamp))
                && stored < before
                && found.is_none_or(|chosen| stored > chosen)
            {
                found = Some(stored);
            }
            !sink.cancelled()
        },
    )?;
    Ok(found)
}

fn selected_history_layouts(
    dataset: &dyn QueryDataset,
    sources: &[HistorySegment],
    datid: u32,
    from: i64,
    to: i64,
    sink: &dyn QuerySink,
) -> Result<BTreeMap<i64, HistoryMoment>, QueryError> {
    let mut by_layout = BTreeMap::<u32, std::collections::BTreeSet<i64>>::new();
    for source in sources {
        if sink.cancelled() {
            break;
        }
        let segment = dataset.open(&source.segment)?;
        for plan in &source.plans {
            let Some(timestamp) = plan.timestamp else {
                continue;
            };
            segment.visit_rows(
                plan.type_id,
                &[timestamp, "datid"],
                0,
                usize::MAX,
                |_ordinal, row| {
                    if sink.cancelled() {
                        return false;
                    }
                    let (Some(stored_datid), Some(stored)) = (
                        unsigned_cell(row.get("datid")),
                        timestamp_cell(row.get(timestamp)),
                    ) else {
                        return true;
                    };
                    if stored_datid == datid && stored <= to {
                        by_layout.entry(plan.type_id).or_default().insert(stored);
                    }
                    true
                },
            )?;
        }
    }
    let mut selected = BTreeMap::<i64, HistoryMoment>::new();
    for (type_id, moments) in &by_layout {
        let mut previous = None;
        for timestamp in moments {
            if (from..=to).contains(timestamp) {
                let candidate = HistoryMoment {
                    type_id: *type_id,
                    previous,
                };
                selected
                    .entry(*timestamp)
                    .and_modify(|chosen| {
                        if candidate.type_id > chosen.type_id {
                            *chosen = candidate;
                        }
                    })
                    .or_insert(candidate);
            }
            previous = Some(*timestamp);
        }
    }
    Ok(selected)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one history scan keeps its exact target, window, and reducer state explicit"
)]
fn scan_history_plan(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    group: RelationGroup,
    datid: u32,
    from: i64,
    to: i64,
    selected: &BTreeMap<i64, HistoryMoment>,
    previous: &mut BTreeMap<(u32, Vec<IdentityCell>), HistoryPrevious>,
    aggregates: &mut BTreeMap<(i64, GroupKey), RelationAggregate>,
    sink: &dyn QuerySink,
) -> Result<(), QueryError> {
    if plan.timestamp.is_none() {
        return Ok(());
    }
    let counters = rate_columns(plan);
    let mut chunk = Vec::with_capacity(HISTORY_CHUNK_ROWS);
    let mut failure = None;
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        0,
        usize::MAX,
        |ordinal, row| {
            chunk.push((ordinal, row));
            if chunk.len() == HISTORY_CHUNK_ROWS
                && let Err(error) = process_history_chunk(
                    segment, plan, kind, group, datid, from, to, selected, &counters, previous,
                    aggregates, &mut chunk,
                )
            {
                failure = Some(error);
                return false;
            }
            !sink.cancelled()
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if !sink.cancelled() && !chunk.is_empty() {
        process_history_chunk(
            segment, plan, kind, group, datid, from, to, selected, &counters, previous, aggregates,
            &mut chunk,
        )?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bounded chunk shares request-scoped predecessor and aggregate state"
)]
fn process_history_chunk(
    segment: &Segment,
    plan: &Plan,
    kind: RelationKind,
    group: RelationGroup,
    datid: u32,
    from: i64,
    to: i64,
    selected: &BTreeMap<i64, HistoryMoment>,
    counters: &[&'static str],
    previous: &mut BTreeMap<(u32, Vec<IdentityCell>), HistoryPrevious>,
    aggregates: &mut BTreeMap<(i64, GroupKey), RelationAggregate>,
    chunk: &mut Vec<(u64, Row)>,
) -> Result<(), QueryError> {
    let dictionary = chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        if unsigned_cell(row.get("datid")) != Some(datid) || !plan.matches(&row, &dictionary) {
            continue;
        }
        let (Some(timestamp), Some(identity)) = (
            plan.timestamp
                .and_then(|name| timestamp_cell(row.get(name))),
            identity_of(plan, &row),
        ) else {
            continue;
        };
        let history_key = (plan.type_id, identity);
        let before = previous.get(&history_key);
        if before.is_some_and(|stored| stored.timestamp >= timestamp) {
            continue;
        }
        let moment = selected.get(&timestamp).copied();
        let required_previous = moment
            .filter(|moment| moment.type_id == plan.type_id)
            .and_then(|moment| moment.previous);
        let elapsed = required_previous
            .and_then(|stored| timestamp.checked_sub(stored))
            .filter(|elapsed| *elapsed > 0);
        let exact_before = before.filter(|stored| Some(stored.timestamp) == required_previous);
        if (from..=to).contains(&timestamp)
            && moment.is_some_and(|moment| moment.type_id == plan.type_id)
            && let Some(key) = GroupKey::from_row(kind, group, &row, &dictionary)?
        {
            let source_row = RelationSource {
                segment_id: segment.id(),
                context_index: 0,
                ordinal,
                type_id: plan.type_id,
                timestamp,
            };
            aggregates
                .entry((timestamp, key.clone()))
                .or_insert_with(|| RelationAggregate::new(key, source_row))
                .add(
                    kind,
                    plan,
                    &row,
                    exact_before.map(|stored| &stored.readings),
                    elapsed,
                    &dictionary,
                    source_row,
                )?;
        }
        let readings = counters
            .iter()
            .filter_map(|name| row.get(name).cloned().map(|value| (*name, value)))
            .collect();
        previous.insert(
            history_key,
            HistoryPrevious {
                timestamp,
                readings,
            },
        );
    }
    Ok(())
}

struct RelationRow {
    key: GroupKey,
    metrics: BTreeMap<String, Option<Metric>>,
    from: Option<i64>,
    to: Option<i64>,
}

fn relation_layout(
    logical_name: &str,
    kind: RelationKind,
    group: RelationGroup,
    selected: &[String],
) -> Result<Vec<u8>, QueryError> {
    let available = kind.fields(group);
    let columns = selected
        .iter()
        .filter_map(|name| available.iter().find(|field| field.name == name))
        .map(|field| {
            json!({
                "name": field.name,
                "kind": kind_name(field.kind),
                "unit": field.unit.unwrap_or("none"),
                "nullable": true,
            })
        })
        .collect::<Vec<_>>();
    record(json!({
        "record": "relation_layout",
        "logical_name": logical_name,
        "group": group.as_str(),
        "columns": columns,
    }))
}

fn relation_record(
    logical_name: &str,
    kind: RelationKind,
    group: RelationGroup,
    row: &RelationRow,
) -> Result<Vec<u8>, QueryError> {
    let values = relation_values(&row.metrics);
    record(json!({
        "record": "relation",
        "logical_name": logical_name,
        "group": group.as_str(),
        "key": row.key.json(kind, group),
        "values": values,
        "sample_from": row.from.map(|value| value.to_string()),
        "sample_to": row.to.map(|value| value.to_string()),
        "source": null,
    }))
}

fn relation_values(metrics: &BTreeMap<String, Option<Metric>>) -> Map<String, Value> {
    metrics
        .iter()
        .map(|(name, metric)| {
            (
                name.clone(),
                metric.as_ref().map_or(Value::Null, Metric::json),
            )
        })
        .collect()
}

fn add_ordered(left: Option<OrderedNumber>, right: OrderedNumber) -> Option<OrderedNumber> {
    match (left, right) {
        (None, right) => Some(right),
        (Some(OrderedNumber::Integer(left)), OrderedNumber::Integer(right)) => {
            left.checked_add(right).map(OrderedNumber::Integer)
        }
        (Some(left), right) => {
            let sum = left.as_f64() + right.as_f64();
            sum.is_finite().then_some(OrderedNumber::Float(sum))
        }
    }
}

fn ordered_cell(cell: &Cell) -> Option<OrderedNumber> {
    match cell {
        Cell::I16(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::I64(value) | Cell::Ts(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U32(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::U64(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(value) if value.is_finite() => Some(OrderedNumber::Float(*value)),
        Cell::Bool(value) => Some(OrderedNumber::Integer(i128::from(*value))),
        Cell::F64(_) | Cell::StrId(_) | Cell::ListI32(_) | Cell::Null => None,
    }
}

fn counter_delta(now: &Cell, earlier: &Cell) -> Option<OrderedNumber> {
    let exact = match (now, earlier) {
        (Cell::I16(now), Cell::I16(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I32(now), Cell::I32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::I64(now) | Cell::Ts(now), Cell::I64(earlier) | Cell::Ts(earlier)) => {
            i128::from(*now) - i128::from(*earlier)
        }
        (Cell::U32(now), Cell::U32(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::U64(now), Cell::U64(earlier)) => i128::from(*now) - i128::from(*earlier),
        (Cell::F64(now), Cell::F64(earlier)) => {
            let delta = now - earlier;
            return (delta >= 0.0 && delta.is_finite()).then_some(OrderedNumber::Float(delta));
        }
        _ => return None,
    };
    (exact >= 0).then_some(OrderedNumber::Integer(exact))
}

fn rate_columns(plan: &Plan) -> Vec<&'static str> {
    plan.fields
        .iter()
        .filter_map(|field| field.column)
        .filter(|column| {
            plan.contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
        })
        .collect()
}

fn identity_of(plan: &Plan, row: &Row) -> Option<Vec<IdentityCell>> {
    if plan.contract.identity.is_empty() {
        return Some(Vec::new());
    }
    let mut identity = Vec::with_capacity(plan.contract.identity.len());
    for name in plan.contract.identity {
        identity.push(identity_cell(row.get(name)?));
    }
    Some(identity)
}

fn identity_cell(stored: &Cell) -> IdentityCell {
    match stored {
        Cell::Null => IdentityCell::Null,
        Cell::I16(value) => IdentityCell::I16(*value),
        Cell::I32(value) => IdentityCell::I32(*value),
        Cell::I64(value) => IdentityCell::I64(*value),
        Cell::Ts(value) => IdentityCell::Ts(*value),
        Cell::U32(value) => IdentityCell::U32(*value),
        Cell::U64(value) => IdentityCell::U64(*value),
        Cell::F64(value) => IdentityCell::F64(value.to_bits()),
        Cell::Bool(value) => IdentityCell::Bool(*value),
        Cell::ListI32(value) => IdentityCell::ListI32(value.clone()),
        Cell::StrId(id) => IdentityCell::StrId(*id),
    }
}

fn counter_input(
    plan: &Plan,
    row: &Row,
    before: Option<&CounterReadings>,
    name: &'static str,
    structural: bool,
) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    let (Some(now), Some(earlier)) = (row.get(name), before.and_then(|values| values.get(name)))
    else {
        return Input::Unavailable;
    };
    if structural && matches!((now, earlier), (Cell::Null, Cell::Null)) {
        return Input::Neutral;
    }
    counter_delta(now, earlier).map_or(Input::Unavailable, Input::Value)
}

fn gauge_input(plan: &Plan, row: &Row, name: &'static str, structural: bool) -> Input {
    if plan.contract.column(name).is_none() {
        return Input::Unavailable;
    }
    match row.get(name) {
        Some(Cell::Null) if structural => Input::Neutral,
        Some(cell) => ordered_cell(cell).map_or(Input::Unavailable, Input::Value),
        None => Input::Unavailable,
    }
}

fn text_cell(stored: Option<&Cell>, dictionary: &Dictionary) -> Result<Option<String>, QueryError> {
    let Some(Cell::StrId(id)) = stored else {
        return Ok(None);
    };
    let bytes = dictionary
        .resolve(*id)
        .map(Resolved::stored_bytes)
        .ok_or(QueryError::BadCursor)?;
    String::from_utf8(bytes.to_vec())
        .map(Some)
        .map_err(|error| QueryError::Unreadable(Box::new(error)))
}

const fn unsigned_cell(stored: Option<&Cell>) -> Option<u32> {
    match stored {
        Some(Cell::U32(value)) => Some(*value),
        _ => None,
    }
}

fn integer_cell(stored: Option<&Cell>) -> Option<i128> {
    ordered_cell(stored?).and_then(|value| match value {
        OrderedNumber::Integer(value) => Some(value),
        OrderedNumber::Float(_) => None,
    })
}

const fn timestamp_cell(stored: Option<&Cell>) -> Option<i64> {
    match stored {
        Some(Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

const fn bool_cell(stored: Option<&Cell>) -> Option<bool> {
    match stored {
        Some(Cell::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn number_is_zero(value: OrderedNumber) -> bool {
    match value {
        OrderedNumber::Integer(value) => value == 0,
        OrderedNumber::Float(value) => value == 0.0,
    }
}

const fn kind_name(kind: Kind) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(datid: u32) -> GroupKey {
        GroupKey(GroupKeyValue::Index {
            datid,
            datname: format!("db{datid}"),
            schemaname: "public".to_owned(),
            relid: 10 + datid,
            relname: format!("table{datid}"),
            indexrelid: 20 + datid,
            indexrelname: format!("index{datid}"),
        })
    }

    fn table_key(datid: u32) -> GroupKey {
        GroupKey(GroupKeyValue::Table {
            datid,
            datname: format!("db{datid}"),
            schemaname: "public".to_owned(),
            relid: 10 + datid,
            relname: format!("table{datid}"),
        })
    }

    const fn source(context_index: usize, ordinal: u64) -> RelationSource {
        RelationSource::new(7, context_index, ordinal, 1_014_004, 42)
    }

    fn aggregate() -> RelationAggregate {
        RelationAggregate::new(key(1), source(2, 3))
    }

    fn add_rate(rate: &mut RateAggregate, delta: i128) {
        rate.add(Input::Value(OrderedNumber::Integer(delta)), Some(1_000_000));
    }

    fn metric_number(metric: Option<Metric>) -> Value {
        metric.expect("available metric").json()
    }

    #[test]
    fn exact_metric_comparison_preserves_large_ratios() {
        assert_eq!(
            compare_products(&[100, 1_000_000], &[1_048_575, 1_000_000]),
            Ordering::Less
        );
        assert_eq!(
            compare_products(&[u128::MAX, u128::MAX], &[u128::MAX, u128::MAX - 1]),
            Ordering::Greater
        );
        let larger = Metric::rate(RateValue::exact(u128::MAX, u128::MAX - 1));
        let smaller = Metric::rate(RateValue::exact(u128::MAX - 1, u128::MAX));
        assert_eq!(larger.compare(&smaller), Some(Ordering::Greater));
        assert_eq!(smaller.compare(&larger), Some(Ordering::Less));
        assert_eq!(larger.compare(&larger), Some(Ordering::Equal));
    }

    #[test]
    fn relation_contract_rejects_physical_and_cross_section_fields() {
        let sections = vec![TABLES.to_owned()];
        assert!(matches!(
            output_fields(
                &sections,
                RelationGroup::Database,
                &["relid".to_owned()]
            ),
            Err(QueryError::NoSuchColumn(name)) if name == "relid"
        ));
        assert!(matches!(
            output_fields(
                &[TABLES.to_owned(), INDEXES.to_owned()],
                RelationGroup::Database,
                &[]
            ),
            Err(QueryError::BadFilter(name)) if name == "group"
        ));
    }

    #[test]
    fn object_keys_are_minimal_and_display_identity_is_a_value() {
        let table_key = table_key(7).json(RelationKind::Tables, RelationGroup::Object);
        assert_eq!(
            table_key
                .as_object()
                .expect("table key")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname", "relid", "relname", "schemaname"]
        );
        let index_key = key(7).json(RelationKind::Indexes, RelationGroup::Object);
        assert_eq!(
            index_key
                .as_object()
                .expect("index key")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            [
                "datid",
                "datname",
                "indexrelid",
                "indexrelname",
                "relid",
                "relname",
                "schemaname"
            ]
        );
        let output = output_fields(
            &[INDEXES.to_owned()],
            RelationGroup::Object,
            &[
                "datid".to_owned(),
                "indexrelid".to_owned(),
                "indexrelname".to_owned(),
                "relid".to_owned(),
                "relname".to_owned(),
            ],
        )
        .expect("valid identity fields");
        assert!(output.is_empty(), "identity values are emitted in the key");

        assert!(matches!(
            output_fields(
                &[INDEXES.to_owned()],
                RelationGroup::Object,
                &["indexdef".to_owned()]
            ),
            Err(QueryError::NoSuchColumn(name)) if name == "indexdef"
        ));
    }

    #[test]
    fn database_schema_and_object_keys_keep_database_scope() {
        let first = key(1);
        let second = key(2);
        assert_eq!(
            first.metric("schemaname").expect("schema metric").json(),
            second.metric("schemaname").expect("schema metric").json()
        );
        assert_ne!(
            first, second,
            "the same schema name in two databases is distinct"
        );

        assert_eq!(
            first
                .json(RelationKind::Tables, RelationGroup::Database)
                .as_object()
                .expect("database key")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname"]
        );
        assert_eq!(
            first
                .json(RelationKind::Tables, RelationGroup::Schema)
                .as_object()
                .expect("schema key")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["datid", "datname", "schemaname"]
        );
        assert_ne!(
            first.json(RelationKind::Tables, RelationGroup::Object),
            second.json(RelationKind::Tables, RelationGroup::Object)
        );
    }

    #[test]
    fn aggregate_contract_has_explicit_counts_and_timestamp_semantics() {
        let table = RelationKind::Tables.fields(RelationGroup::Schema);
        assert_eq!(
            table
                .iter()
                .find(|field| field.name() == "table_count")
                .and_then(|field| field.unit()),
            Some("count")
        );
        assert!(
            table
                .iter()
                .any(|field| field.name() == "last_vacuum_oldest")
        );
        assert!(
            table
                .iter()
                .any(|field| field.name() == "last_vacuum_never_count")
        );
        assert!(
            table
                .iter()
                .any(|field| field.name() == "toast_last_autovacuum_latest")
        );
        assert!(!table.iter().any(|field| field.name() == "tablespace"));
        assert_eq!(
            RelationKind::Tables.fields(RelationGroup::Tablespace)[0].name(),
            "tablespace"
        );

        let index = RelationKind::Indexes.fields(RelationGroup::Database);
        for name in [
            "index_count",
            "last_idx_scan_never_count",
            "no_scan_count",
            "known_scan_count",
            "invalid_count",
            "unready_count",
            "unique_count",
            "primary_count",
            "exclusion_count",
        ] {
            assert_eq!(
                index
                    .iter()
                    .find(|field| field.name() == name)
                    .and_then(|field| field.unit()),
                Some("count"),
                "{name}"
            );
        }
        for name in [
            "idx_scan",
            "idx_tup_read",
            "idx_tup_fetch",
            "idx_blks_read",
            "idx_blks_hit",
        ] {
            assert_eq!(
                index
                    .iter()
                    .find(|field| field.name() == name)
                    .and_then(|field| field.unit()),
                Some("per_second"),
                "{name}"
            );
        }
        assert!(!index.iter().any(|field| field.name() == "indisvalid"));
        assert_eq!(
            RelationKind::Indexes.fields(RelationGroup::Tablespace)[0].name(),
            "tablespace"
        );
    }

    #[test]
    fn unavailable_values_are_explicit_nulls() {
        let metrics = BTreeMap::from([
            ("available".to_owned(), Some(Metric::integer(7))),
            ("unavailable".to_owned(), None),
        ]);
        let values = relation_values(&metrics);
        assert_eq!(
            values.get("available"),
            Some(&Value::String("7".to_owned()))
        );
        assert_eq!(values.get("unavailable"), Some(&Value::Null));
    }

    #[test]
    fn additive_rates_and_ratios_sum_each_objects_own_interval() {
        let mut aggregate = aggregate();
        let mut sequential = RateAggregate::default();
        sequential.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
        sequential.add(Input::Value(OrderedNumber::Integer(20)), Some(5_000_000));
        aggregate.rates.insert("seq_scan", sequential);
        let mut indexed = RateAggregate::default();
        indexed.add(Input::Value(OrderedNumber::Integer(10)), Some(10_000_000));
        indexed.add(Input::Value(OrderedNumber::Integer(5)), Some(5_000_000));
        aggregate.rates.insert("idx_scan", indexed);
        assert_eq!(
            metric_number(sequential.metric()),
            json!(5.0),
            "10/10s + 20/5s is five scans per second"
        );
        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Database,
                "sequential_share_pct",
            )),
            json!(500.0 / 7.0),
            "the percentage is recomputed from the summed 5/s and 2/s operands"
        );
        let exact = sequential.metric().expect("exact rate");
        assert_eq!(
            exact.compare(&Metric::rate(RateValue::exact(4, 1_000_000))),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn table_cuts_recompute_from_exact_operands() {
        let mut aggregate = aggregate();
        for (name, delta) in [
            ("seq_scan", 2),
            ("idx_scan", 6),
            ("seq_tup_read", 20),
            ("idx_tup_fetch", 60),
            ("n_tup_ins", 1),
            ("n_tup_upd", 3),
            ("n_tup_del", 6),
            ("n_tup_hot_upd", 2),
            ("n_tup_newpage_upd", 1),
            ("heap_blks_read", 1),
            ("heap_blks_hit", 9),
            ("idx_blks_read", 2),
            ("idx_blks_hit", 8),
        ] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, delta);
            aggregate.rates.insert(name, rate);
        }
        for name in [
            "toast_blks_read",
            "toast_blks_hit",
            "tidx_blks_read",
            "tidx_blks_hit",
        ] {
            aggregate.rates.insert(name, RateAggregate::default());
        }
        for (name, value) in [
            ("main_fork_bytes", 800),
            ("toast_bytes", 200),
            ("toast_n_live_tup", 90),
            ("toast_n_dead_tup", 10),
        ] {
            aggregate
                .gauges
                .insert(name, Availability::Value(OrderedNumber::Integer(value)));
        }

        let table = |name: &'static str| {
            aggregate
                .metric(RelationKind::Tables, RelationGroup::Database, name)
                .expect("available table cut")
                .json()
        };
        assert_eq!(table("tuple_throughput"), json!(80.0));
        assert_eq!(table("dml_total"), json!(10.0));
        assert_eq!(table("insert_share_pct"), json!(10.0));
        assert_eq!(table("update_share_pct"), json!(30.0));
        assert_eq!(table("delete_share_pct"), json!(60.0));
        assert_eq!(table("displayed_storage_bytes"), json!("1000"));
        assert_eq!(table("toast_share_pct"), json!(20.0));
        assert_eq!(table("toast_dead_pct"), json!(10.0));
        let assert_close = |name: &'static str, expected: f64| {
            let value = table(name);
            let actual = value.as_f64().expect("numeric table cut");
            assert!((actual - expected).abs() < 1e-12, "{name}: {actual}");
        };
        assert_close("heap_buffer_hit_pct", 90.0);
        assert_close("index_buffer_hit_pct", 80.0);
        assert_close("buffer_hit_pct", 85.0);
        assert!(
            aggregate
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Database,
                    "toast_buffer_hit_pct"
                )
                .is_none()
        );
    }

    #[test]
    fn size_metrics_use_the_authoritative_reducer_and_exact_boundaries() {
        let mut table = aggregate();
        table.gauges.insert(
            "main_fork_bytes",
            Availability::Value(OrderedNumber::Integer(80_000_000)),
        );
        table.gauges.insert(
            "toast_bytes",
            Availability::Value(OrderedNumber::Integer(25_000_000)),
        );
        let size = table
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes",
            )
            .expect("table size");
        assert_eq!(
            size.compare(&Metric::integer(100_000_000)),
            Some(Ordering::Greater)
        );

        table.gauges.insert(
            "toast_bytes",
            Availability::Value(OrderedNumber::Integer(20_000_000)),
        );
        let size = table
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes",
            )
            .expect("table size");
        assert_eq!(
            size.compare(&Metric::integer(100_000_000)),
            Some(Ordering::Equal)
        );
        assert_eq!(
            size.compare(&Metric::integer(99_999_999)),
            Some(Ordering::Greater)
        );
        assert_eq!(
            size.compare(&Metric::integer(100_000_001)),
            Some(Ordering::Less)
        );

        table
            .gauges
            .insert("toast_bytes", Availability::Unavailable);
        assert!(
            table
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "displayed_storage_bytes",
                )
                .is_none()
        );

        let mut index = aggregate();
        index.gauges.insert(
            "main_fork_bytes",
            Availability::Value(OrderedNumber::Integer(100_000_000)),
        );
        let size = index
            .metric(
                RelationKind::Indexes,
                RelationGroup::Object,
                "main_fork_bytes",
            )
            .expect("index size");
        assert_eq!(
            size.compare(&Metric::integer(100_000_000)),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn structural_absence_is_zero_only_inside_valid_sums() {
        let mut stored = aggregate();
        stored.gauges.insert(
            "main_fork_bytes",
            Availability::Value(OrderedNumber::Integer(800)),
        );
        stored.gauges.insert("toast_bytes", Availability::Empty);
        assert_eq!(
            metric_number(stored.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "displayed_storage_bytes"
            )),
            json!("800")
        );
        assert_eq!(
            metric_number(stored.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "toast_share_pct"
            )),
            json!(0.0)
        );

        for name in ["heap_blks_read", "heap_blks_hit"] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, 0);
            stored.rates.insert(name, rate);
        }
        for name in [
            "idx_blks_read",
            "idx_blks_hit",
            "toast_blks_read",
            "toast_blks_hit",
            "tidx_blks_read",
            "tidx_blks_hit",
        ] {
            stored.rates.insert(name, RateAggregate::default());
        }
        let ratio = stored
            .metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "buffer_hit_pct",
            )
            .expect("exact zero-access ratio");
        assert_eq!(ratio.json(), Value::Null);
        assert!(ratio.compare(&ratio).is_none());

        let mut scans = aggregate();
        let mut sequential = RateAggregate::default();
        add_rate(&mut sequential, 4);
        scans.rates.insert("seq_scan", sequential);
        scans.rates.insert("idx_scan", RateAggregate::default());
        assert_eq!(
            metric_number(scans.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "sequential_share_pct"
            )),
            json!(100.0)
        );

        stored
            .gauges
            .insert("toast_bytes", Availability::Unavailable);
        assert!(
            stored
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "displayed_storage_bytes"
                )
                .is_none()
        );
    }

    #[test]
    fn zero_denominators_are_unavailable_for_table_cuts() {
        let mut aggregate = aggregate();
        for name in [
            "seq_scan",
            "seq_tup_read",
            "idx_scan",
            "idx_tup_fetch",
            "n_tup_ins",
            "n_tup_upd",
            "n_tup_del",
            "n_tup_hot_upd",
            "n_tup_newpage_upd",
            "heap_blks_read",
            "heap_blks_hit",
        ] {
            let mut rate = RateAggregate::default();
            add_rate(&mut rate, 0);
            aggregate.rates.insert(name, rate);
        }
        for name in [
            "n_live_tup",
            "n_dead_tup",
            "main_fork_bytes",
            "toast_bytes",
            "toast_n_live_tup",
            "toast_n_dead_tup",
        ] {
            aggregate
                .gauges
                .insert(name, Availability::Value(OrderedNumber::Integer(0)));
        }

        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "tuple_throughput"
            )),
            json!(0.0)
        );
        assert_eq!(
            metric_number(aggregate.metric(
                RelationKind::Tables,
                RelationGroup::Object,
                "dml_total"
            )),
            json!(0.0)
        );
        for name in [
            "sequential_share_pct",
            "seq_tuples_per_scan",
            "idx_tuples_per_scan",
            "insert_share_pct",
            "update_share_pct",
            "delete_share_pct",
            "hot_pct",
            "new_page_pct",
            "dead_pct",
            "toast_share_pct",
            "toast_dead_pct",
            "heap_buffer_hit_pct",
        ] {
            let metric = aggregate
                .metric(RelationKind::Tables, RelationGroup::Object, name)
                .unwrap_or_else(|| panic!("exact zero-denominator metric {name}"));
            assert_eq!(metric.json(), Value::Null, "{name}");
            assert!(metric.compare(&metric).is_none(), "{name}");
        }
    }

    #[test]
    fn last_scan_never_is_distinct_from_layout_absence() {
        let mut aggregate = aggregate();
        let mut never = TimestampAggregate::default();
        never.add(TimestampObservation::Stored(Some(&Cell::Null)));
        aggregate.timestamps.insert("last_seq_scan", never);
        assert_eq!(
            aggregate
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "last_seq_scan_never",
                )
                .expect("never metric")
                .json(),
            json!(true)
        );

        let mut seen = TimestampAggregate::default();
        seen.add(TimestampObservation::Stored(Some(&Cell::Ts(30))));
        aggregate.timestamps.insert("last_idx_scan", seen);
        for kind in [RelationKind::Tables, RelationKind::Indexes] {
            assert_eq!(
                aggregate
                    .metric(kind, RelationGroup::Object, "last_idx_scan_never")
                    .expect("seen metric")
                    .json(),
                json!(false)
            );
        }

        let mut absent = TimestampAggregate::default();
        absent.add(TimestampObservation::Unavailable);
        aggregate.timestamps.insert("last_seq_scan", absent);
        assert!(
            aggregate
                .metric(
                    RelationKind::Tables,
                    RelationGroup::Object,
                    "last_seq_scan_never"
                )
                .is_none()
        );
    }

    #[test]
    fn reset_and_layout_unknown_poison_but_structural_null_is_neutral() {
        let mut sum = Availability::default();
        sum.add(Input::Neutral);
        assert!(sum.value().is_none(), "all structural N/A stays null");
        sum.add(Input::Value(OrderedNumber::Integer(8)));
        assert!(matches!(sum.value(), Some(OrderedNumber::Integer(8))));
        sum.add(Input::Unavailable);
        assert!(
            sum.value().is_none(),
            "reset/layout absence poisons the sum"
        );

        let mut rate = RateAggregate::default();
        rate.add(Input::Neutral, None);
        assert!(rate.metric().is_none(), "all structural N/A stays null");
        add_rate(&mut rate, 8);
        rate.add(Input::Unavailable, Some(1_000_000));
        assert!(
            rate.metric().is_none(),
            "one reset poisons an aggregate rate"
        );

        let mut missing_elapsed = RateAggregate::default();
        missing_elapsed.add(Input::Value(OrderedNumber::Integer(1)), None);
        assert!(missing_elapsed.metric().is_none());
    }

    #[test]
    fn ages_take_the_maximum_ignore_partition_na_and_reject_layout_absence() {
        let mut maximum = MaximumAggregate::default();
        maximum.add(true, Some(&Cell::Null));
        maximum.add(true, Some(&Cell::I64(41)));
        maximum.add(true, Some(&Cell::I64(17)));
        assert_eq!(maximum.exact(), Some(41));

        let mut all_na = MaximumAggregate::default();
        all_na.add(true, Some(&Cell::Null));
        assert_eq!(all_na.exact(), None);

        maximum.add(false, None);
        assert_eq!(maximum.exact(), None, "layout absence poisons the maximum");
    }

    #[test]
    fn timestamps_keep_oldest_latest_and_explicit_never() {
        let mut timestamp = TimestampAggregate::default();
        timestamp.add(TimestampObservation::Stored(Some(&Cell::Ts(30))));
        timestamp.add(TimestampObservation::Stored(Some(&Cell::Null)));
        timestamp.add(TimestampObservation::Stored(Some(&Cell::Ts(10))));
        assert!(timestamp.exact());
        assert_eq!(timestamp.oldest, Some(10));
        assert_eq!(timestamp.latest, Some(30));
        assert_eq!(timestamp.never, 1);

        let mut toast = TimestampAggregate::default();
        toast.add(TimestampObservation::NotApplicable);
        assert!(!toast.exact(), "all no-TOAST rows stay unavailable");
        toast.add(TimestampObservation::Stored(Some(&Cell::Null)));
        assert!(toast.exact());
        assert_eq!(toast.never, 1);

        let mut absent = TimestampAggregate::default();
        absent.add(TimestampObservation::Unavailable);
        assert!(!absent.exact());
    }

    #[test]
    fn index_flags_and_each_objects_scan_delta_are_counted() {
        let mut scans = BoolAggregate::default();
        scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
        scans.add_scan(Input::Value(OrderedNumber::Integer(5)));
        scans.add_scan(Input::Value(OrderedNumber::Integer(0)));
        assert!(scans.exact());
        assert_eq!((scans.known, scans.truthy), (3, 2));

        let mut aggregate = aggregate();
        let mut valid = BoolAggregate::default();
        valid.add(Some(&Cell::Bool(true)));
        valid.add(Some(&Cell::Bool(false)));
        aggregate.flags.insert("indisvalid", valid);
        assert_eq!(
            aggregate
                .flag_count("indisvalid", false)
                .expect("invalid count")
                .json(),
            json!("1")
        );

        aggregate.state_severity = Some(2);
        assert_eq!(
            aggregate
                .metric(
                    RelationKind::Indexes,
                    RelationGroup::Database,
                    "state_severity",
                )
                .expect("state severity")
                .json(),
            json!("2")
        );

        scans.add_scan(Input::Unavailable);
        assert!(!scans.exact(), "unknown scan delta poisons the count");
    }

    #[test]
    fn reltuples_unknown_poisons_the_sum_and_zero_remains_exact() {
        let mut aggregate = Availability::default();
        aggregate.add(Input::Value(OrderedNumber::Integer(0)));
        aggregate.add(Input::Value(OrderedNumber::Integer(12)));
        assert!(matches!(
            aggregate.value(),
            Some(OrderedNumber::Integer(12))
        ));
        aggregate.add(Input::Unavailable);
        assert!(aggregate.value().is_none());
    }
}
