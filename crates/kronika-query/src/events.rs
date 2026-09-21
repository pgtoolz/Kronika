//! Shared recorded-event query, decoding, grouping, and result contract.

mod group;
mod scan;
mod source;

use std::collections::BTreeMap;
use std::sync::Arc;

use group::{EventGroups, SlowThreshold};
use kronika_reader::SegmentKind;
use scan::{carries_selected, collect_section};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::render::record;
use super::row_key::{self, DetailLocator};
use super::time::TimeRange;

use crate::request::Filter;
use crate::{
    DatasetSegment, QueryContext, QueryDataset, QueryError, QuerySink, QueryStability,
    SegmentBounds, SegmentSelection,
};

/// Maximum accepted Events time-window width, in microseconds.
pub const MAX_EVENTS_WINDOW_MICROS: i64 = 3_600_000_000;
/// Maximum accepted group or occurrence count.
pub const MAX_EVENTS_LIMIT: usize = 5_000;
const ROW_CHUNK_ROWS: usize = 512;
const MINUTE_COLUMNS: usize = 60;
const MINUTE_MICROS: i64 = 60_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
/// Public Events output shape.
pub enum EventsRepresentation {
    /// Collated event groups.
    Groups,
    /// Individual physical occurrences.
    Occurrences,
}

impl EventsRepresentation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Groups => "groups",
            Self::Occurrences => "occurrences",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub(crate) enum EventSource {
    #[serde(rename = "pg_log_errors")]
    Errors,
    #[serde(rename = "pg_log_checkpoints")]
    Checkpoints,
    #[serde(rename = "pg_log_autovacuum")]
    Autovacuum,
    #[serde(rename = "pg_log_slow_queries")]
    SlowQueries,
    #[serde(rename = "pg_log_lock_waits")]
    LockWaits,
    #[serde(rename = "pg_log_temp_files")]
    TempFiles,
    #[serde(rename = "pg_log_lifecycle")]
    Lifecycle,
    #[serde(rename = "pgbouncer_events")]
    Pgbouncer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validated recorded-event query.
pub struct EventsQuery {
    range: TimeRange,
    sources: Vec<EventSource>,
    representation: EventsRepresentation,
    limit: usize,
}

impl EventsQuery {
    /// Validate and deduplicate an Events selection in caller order.
    ///
    /// Omitting `requested` selects every source valid for the chosen
    /// representation.
    ///
    /// # Errors
    ///
    /// Returns an exact limit or source-vocabulary error.
    pub fn normalize(
        range: TimeRange,
        requested: Option<Vec<String>>,
        representation: EventsRepresentation,
        limit: usize,
    ) -> Result<Self, EventsQueryError> {
        if !(1..=MAX_EVENTS_LIMIT).contains(&limit) {
            return Err(EventsQueryError::Limit(limit));
        }
        let valid = match representation {
            EventsRepresentation::Groups => &EventSource::GROUPS[..],
            EventsRepresentation::Occurrences => &EventSource::OCCURRENCES[..],
        };
        let mut sources = Vec::new();
        for source in requested.unwrap_or_else(|| {
            valid
                .iter()
                .map(|source| source.as_str().to_owned())
                .collect()
        }) {
            let parsed = EventSource::parse(&source)
                .filter(|source| valid.contains(source))
                .ok_or_else(|| EventsQueryError::Source {
                    source,
                    valid: valid
                        .iter()
                        .map(|source| source.as_str().to_owned())
                        .collect(),
                })?;
            if !sources.contains(&parsed) {
                sources.push(parsed);
            }
        }
        Ok(Self {
            range,
            sources,
            representation,
            limit,
        })
    }

    /// Validated half-open recorded-time range.
    #[must_use]
    pub const fn range(&self) -> TimeRange {
        self.range
    }

    /// Requested result shape.
    #[must_use]
    pub const fn representation(&self) -> EventsRepresentation {
        self.representation
    }

    /// Maximum number of emitted groups or occurrences.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Deduplicated source names in caller order.
    pub fn sources(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.sources.iter().map(|source| source.as_str())
    }
}

/// Invalid semantic Events selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventsQueryError {
    /// Requested result limit is outside `1..=MAX_EVENTS_LIMIT`.
    Limit(usize),
    /// A source name is not valid for the chosen representation.
    Source {
        /// Rejected source name.
        source: String,
        /// Complete valid vocabulary in stable order.
        valid: Vec<String>,
    },
}

impl EventsQueryError {
    /// Valid source options when this is a source-name error.
    #[must_use]
    pub fn valid_options(&self) -> Vec<String> {
        match self {
            Self::Source { valid, .. } => valid.clone(),
            Self::Limit(_) => Vec::new(),
        }
    }
}

impl std::fmt::Display for EventsQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit(limit) => write!(
                f,
                "limit must be between 1 and {MAX_EVENTS_LIMIT}, got {limit}"
            ),
            Self::Source { source, valid } => {
                write!(f, "unknown source {source:?}: valid sources are {valid:?}")
            }
        }
    }
}

impl std::error::Error for EventsQueryError {}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "representation", rename_all = "snake_case")]
/// Typed result used by recorded-data adapters.
pub enum EventsResult {
    /// Collated event groups.
    Groups {
        /// Groups in stable semantic rank order.
        groups: Vec<EventGroup>,
        /// Whether more groups matched than the requested limit.
        truncated: bool,
    },
    /// Individual physical event occurrences.
    Occurrences {
        /// Occurrences in timestamp/source/encounter order.
        occurrences: Vec<EventOccurrence>,
        /// Whether more occurrences matched than the requested limit.
        truncated: bool,
    },
}

impl EventsResult {
    /// Result shape.
    #[must_use]
    pub const fn representation(&self) -> EventsRepresentation {
        match self {
            Self::Groups { .. } => EventsRepresentation::Groups,
            Self::Occurrences { .. } => EventsRepresentation::Occurrences,
        }
    }

    /// Whether the normalized limit omitted matches.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        match self {
            Self::Groups { truncated, .. } | Self::Occurrences { truncated, .. } => *truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventDataRow {
    pub(crate) segment_id: i64,
    pub(crate) type_id: u32,
    pub(crate) row_ordinal: u64,
    pub(crate) timestamp: i64,
    pub(crate) identity: row_key::RowIdentity,
    pub(crate) values: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventTier {
    Critical,
    Notable,
    Routine,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// One collated recorded-event group.
pub struct EventGroup {
    pub(crate) key: String,
    pub(crate) section: String,
    pub(crate) tier: EventTier,
    pub(crate) label: Option<String>,
    pub(crate) count: f64,
    pub(crate) first_ts: i64,
    pub(crate) last_ts: i64,
    pub(crate) representative_ts: i64,
    pub(crate) minutes: Vec<f64>,
    pub(crate) stat: EventStat,
    #[serde(rename = "detail_locator")]
    pub(crate) detail_locator: DetailLocator,
}

impl EventGroup {
    /// Encode the stable representative row as an opaque detail reference.
    ///
    /// # Errors
    ///
    /// Returns an explanation when the internal locator is invalid.
    pub fn detail_ref(&self) -> Result<String, String> {
        self.detail_locator.detail_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub(crate) enum EventStat {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// One recorded physical event occurrence.
pub struct EventOccurrence {
    #[serde(flatten)]
    pub(crate) fields: Map<String, Value>,
    pub(crate) source: String,
    pub(crate) detail_locator: DetailLocator,
}

impl EventOccurrence {
    /// Encode this row as an opaque detail reference.
    ///
    /// # Errors
    ///
    /// Returns an explanation when the internal locator is invalid.
    pub fn detail_ref(&self) -> Result<String, String> {
        self.detail_locator.detail_ref()
    }
}

pub(crate) struct PreparedEvents {
    dataset: Arc<dyn QueryDataset>,
    segments: Vec<DatasetSegment>,
    query: EventsQuery,
    validator_shape: String,
}

struct RetainedOccurrence {
    source: EventSource,
    row: EventDataRow,
}

struct OccurrenceAccumulator {
    limit: usize,
    encounters: Vec<u64>,
    rows: BTreeMap<(i64, usize, u64), RetainedOccurrence>,
}

impl OccurrenceAccumulator {
    fn new(query: &EventsQuery) -> Self {
        Self {
            limit: query.limit,
            encounters: vec![0; query.sources.len()],
            rows: BTreeMap::new(),
        }
    }

    fn observe(&mut self, source_rank: usize, source: EventSource, row: EventDataRow) {
        let encounter = self.encounters[source_rank];
        self.encounters[source_rank] = encounter.saturating_add(1);
        let key = (row.timestamp, source_rank, encounter);
        let capacity = self.limit.saturating_add(1);
        if self.rows.len() == capacity {
            if self
                .rows
                .last_key_value()
                .is_some_and(|(last, _row)| key >= *last)
            {
                return;
            }
            self.rows.pop_last();
        }
        self.rows.insert(key, RetainedOccurrence { source, row });
    }

    fn finish(self) -> EventsResult {
        let truncated = self.rows.len() > self.limit;
        let occurrences = self
            .rows
            .into_values()
            .take(self.limit)
            .map(|retained| occurrence(retained.source, retained.row))
            .collect();
        EventsResult::Occurrences {
            occurrences,
            truncated,
        }
    }
}

pub(crate) fn prepare(
    dataset: Arc<dyn QueryDataset>,
    query: EventsQuery,
) -> Result<PreparedEvents, QueryError> {
    let listing = {
        let catalog = dataset.catalog()?;
        catalog.segments(SegmentSelection::new(SegmentBounds::half_open(
            query.range.from,
            query.range.to_exclusive,
        )))?
    };
    let mut segments = listing.segments;
    segments.retain(|segment| {
        segment.max_ts() >= query.range.from && segment.min_ts() < query.range.to_exclusive
    });
    segments.sort_by_key(DatasetSegment::min_ts);
    let validator_shape = format!("{query:?}");
    Ok(PreparedEvents {
        dataset,
        segments,
        query,
        validator_shape,
    })
}

/// Run a recorded-event query and return its typed result.
///
/// # Errors
///
/// Returns a semantic, decoding, cancellation, or captured-source error.
pub fn execute_events(
    context: &QueryContext,
    query: EventsQuery,
    sink: &dyn QuerySink,
) -> Result<EventsResult, QueryError> {
    prepare(Arc::clone(&context.dataset), query)?.execute(sink)
}

impl PreparedEvents {
    pub(crate) fn stability(&self) -> QueryStability {
        if self.segments.is_empty() {
            QueryStability::Revalidate
        } else if self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished)
        {
            QueryStability::Immutable
        } else {
            QueryStability::Mutable
        }
    }

    pub(crate) fn validator_input(&self) -> Option<(&str, &str, &[DatasetSegment])> {
        (self.stability() == QueryStability::Immutable).then_some((
            "events",
            self.validator_shape.as_str(),
            self.segments.as_slice(),
        ))
    }

    pub(crate) fn execute(self, sink: &dyn QuerySink) -> Result<EventsResult, QueryError> {
        match self.query.representation {
            EventsRepresentation::Groups => self.execute_groups(sink),
            EventsRepresentation::Occurrences => self.execute_occurrences(sink),
        }
    }

    fn execute_groups(self, sink: &dyn QuerySink) -> Result<EventsResult, QueryError> {
        let mut groups = EventGroups::new(self.query.range.from);
        let needs_threshold = self.query.representation == EventsRepresentation::Groups
            && self.query.sources.contains(&EventSource::SlowQueries);
        let mut threshold = SlowThreshold::default();

        for segment_ref in &self.segments {
            if sink.cancelled() {
                return Err(QueryError::Cancelled);
            }
            if !carries_selected(segment_ref, &self.query.sources, needs_threshold) {
                continue;
            }
            let segment = self.dataset.open(segment_ref)?;
            for source in &self.query.sources {
                let mut observe = |row| {
                    groups.observe(*source, row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    source.as_str(),
                    source.group_fields(),
                    &[],
                    self.query.range,
                    &mut observe,
                    sink,
                )?;
            }
            if needs_threshold {
                let mut observe = |row| {
                    threshold.observe(&row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    "pg_settings",
                    &["name", "setting", "unit"],
                    &[Filter {
                        column: "name".to_owned(),
                        value: "log_min_duration_statement".to_owned(),
                    }],
                    self.query.range,
                    &mut observe,
                    sink,
                )?;
            }
        }
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        groups_result(&self.query, groups, threshold.finish())
    }

    fn execute_occurrences(self, sink: &dyn QuerySink) -> Result<EventsResult, QueryError> {
        let mut occurrences = OccurrenceAccumulator::new(&self.query);
        for segment_ref in &self.segments {
            if sink.cancelled() {
                return Err(QueryError::Cancelled);
            }
            if !carries_selected(segment_ref, &self.query.sources, false) {
                continue;
            }
            let segment = self.dataset.open(segment_ref)?;
            for (source_rank, source) in self.query.sources.iter().copied().enumerate() {
                let mut observe = |row| {
                    occurrences.observe(source_rank, source, row);
                    Ok(())
                };
                collect_section(
                    &segment,
                    segment_ref.id(),
                    source.as_str(),
                    source.occurrence_fields(),
                    &[],
                    self.query.range,
                    &mut observe,
                    sink,
                )?;
            }
        }
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        Ok(occurrences.finish())
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let result = self.execute(sink)?;
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "events",
                "representation": result.representation().as_str(),
                "truncated": result.truncated(),
            }))?)
        {
            return Ok(());
        }
        match result {
            EventsResult::Groups { groups, .. } => emit_groups(groups, sink),
            EventsResult::Occurrences { occurrences, .. } => emit_occurrences(occurrences, sink),
        }
    }
}

fn emit_groups(groups: Vec<EventGroup>, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
    for group in groups {
        let value = public_item("event_group", &group, &group.detail_locator)?;
        if !sink.record(record(value)?) {
            break;
        }
    }
    Ok(())
}

fn emit_occurrences(
    occurrences: Vec<EventOccurrence>,
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    for occurrence in occurrences {
        let value = public_item("event_occurrence", &occurrence, &occurrence.detail_locator)?;
        if !sink.record(record(value)?) {
            break;
        }
    }
    Ok(())
}

fn public_item<T: Serialize>(
    record_name: &str,
    item: &T,
    locator: &DetailLocator,
) -> Result<Value, QueryError> {
    let mut value = serde_json::to_value(item)?;
    let Value::Object(ref mut object) = value else {
        return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "event item did not serialize as an object",
        ))));
    };
    if object.remove("detail_locator").is_none() {
        return Err(QueryError::BadLocator(
            "event item has no detail locator".to_owned(),
        ));
    }
    object.insert(
        "detail_ref".to_owned(),
        Value::String(locator.detail_ref().map_err(QueryError::BadLocator)?),
    );
    object.insert("record".to_owned(), json!(record_name));
    Ok(value)
}

fn groups_result(
    query: &EventsQuery,
    groups: EventGroups,
    threshold_ms: Option<f64>,
) -> Result<EventsResult, QueryError> {
    let mut groups = groups.finish(threshold_ms)?;
    let truncated = groups.len() > query.limit;
    groups.truncate(query.limit);
    Ok(EventsResult::Groups { groups, truncated })
}

fn occurrence(source: EventSource, row: EventDataRow) -> EventOccurrence {
    let mut fields = row.values;
    let detail_locator = row_key::detail_locator(
        source.as_str(),
        row.segment_id,
        row.timestamp,
        row.type_id,
        row.row_ordinal,
        row.identity,
    );
    fields.retain(|field, _| !row_key::is_detail_text(source.as_str(), field));
    label_event_fields(source.as_str(), &mut fields);
    EventOccurrence {
        fields,
        source: source.as_str().to_owned(),
        detail_locator,
    }
}

/// Add stable labels for numeric event vocabulary fields.
pub fn label_event_fields(section: &str, fields: &mut Map<String, Value>) {
    for (field, labels) in event_labels(section) {
        add_map_label(fields, field, labels);
    }
}

fn event_labels(section: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match section {
        "pg_log_errors" => &[
            ("severity", &["error", "fatal", "panic", "warning", "log"]),
            (
                "category",
                &[
                    "lock",
                    "constraint",
                    "serialization",
                    "timeout",
                    "resource",
                    "data_corruption",
                    "system",
                    "connection",
                    "auth",
                    "syntax",
                    "other",
                ],
            ),
        ],
        "pg_log_checkpoints" => &[("phase", &["started", "completed", "too_frequent"])],
        "pg_log_autovacuum" => &[("kind", &["vacuum", "analyze"])],
        "pg_log_lock_waits" => &[("kind", &["waiting", "acquired"])],
        "pg_log_lifecycle" => &[("kind", &["crash", "shutdown", "ready"])],
        "pgbouncer_events" => &[(
            "level",
            &["fatal", "error", "warning", "log", "debug", "noise"],
        )],
        _ => &[],
    }
}

fn add_map_label(fields: &mut Map<String, Value>, field: &str, labels: &[&str]) {
    let Some(index) = fields
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return;
    };
    if let Some(label) = labels.get(index) {
        fields.insert(format!("{field}_label"), json!(label));
    }
}

#[cfg(test)]
#[path = "tests/events.rs"]
mod tests;
