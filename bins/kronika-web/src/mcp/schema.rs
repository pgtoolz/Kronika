//! Output wire models and JSON Schema adaptation for MCP tools.

use std::sync::Arc;

use rmcp::model::JsonObject;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Stable catalog envelope returned by `kronika_list_recorded_sections`.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(super) struct RecordedSectionsOutput {
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
pub(super) struct HeatmapBatchResult {
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
pub(super) enum EventsResult {
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
pub(super) struct EventGroup {
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
    #[serde(rename = "pgbouncer.events", rename_all = "camelCase")]
    Pgbouncer {
        level: f64,
        database: Option<String>,
        username: Option<String>,
        host: Option<String>,
        source_file: Option<String>,
        pid: Option<f64>,
        side: Option<String>,
        port: Option<f64>,
        age_s: Option<f64>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct EventOccurrence {
    #[serde(flatten)]
    fields: Map<String, Value>,
    source: String,
    detail_ref: String,
}

/// Converts the accepted input schema to rmcp's object representation.
pub(super) fn schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let schema = schemars::schema_for!(T);
    into_schema_object(serde_json::to_value(schema).expect("schema serializes to JSON"))
}

/// Output schemas describe serialization, so nullable fields that are always
/// emitted remain required even though they accept JSON null.
pub(super) fn output_schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    into_schema_object(output_schema::<T>())
}

fn output_schema<T: JsonSchema>() -> Value {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    serde_json::to_value(generator.into_root_schema_for::<T>()).expect("schema serializes to JSON")
}

fn into_schema_object(mut value: Value) -> Arc<JsonObject> {
    let object = value
        .as_object_mut()
        .map(std::mem::take)
        .expect("schema is a JSON object");
    Arc::new(object)
}

/// Row fields depend on the referenced section, so only the common envelope
/// and long-text representation can be described statically.
pub(super) fn row_detail_output_schema() -> Arc<JsonObject> {
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
pub(super) fn opaque_output_schema<T: JsonSchema>() -> Arc<JsonObject> {
    let mut value = output_schema::<T>();
    if let Some(definitions) = value.get_mut("$defs").and_then(Value::as_object_mut) {
        definitions.remove("DetailLocator");
    }
    normalize_opaque_schema(&mut value);
    let object = value.as_object_mut().expect("schema is a JSON object");
    object.insert("type".to_owned(), json!("object"));
    into_schema_object(value)
}

fn normalize_opaque_schema(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("description");
            for child in object.values_mut() {
                normalize_opaque_schema(child);
            }

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
        }
        Value::Array(values) => {
            for child in values {
                normalize_opaque_schema(child);
            }
        }
        _ => {}
    }
}
