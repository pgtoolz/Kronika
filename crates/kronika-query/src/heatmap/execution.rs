//! One-pass Heatmap planning, execution, and transport-independent folding.

mod buckets;
mod error;
mod fold;
mod planning;
mod render;
mod stream;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kronika_reader::{Cell, Row, Segment, SegmentKind};
use kronika_registry::{ColumnClass, Unit, logical_section_name};
use planning::{normalize_items, physical_plans, shared_section_specs};
use serde_json::Value;
use stream::stream_grid;

use super::query::{HeatmapBatchQuery, HeatmapItemQuery, HeatmapView};
use super::result::HeatmapBatchResult;

use crate::render::cell;
use crate::row_key;
use crate::statement_scope::CollectorStatements;
use crate::{
    DatasetSegment, QueryDataset, QueryError, QuerySink, QueryStability, SegmentBounds,
    SegmentSelection,
};

const IDENTITY_ALIASES: [(&str, &str); 2] = [("queryid", "query_id"), ("planid", "plan_id")];
type RenderedIds = HashMap<(usize, u64), Value>;

#[derive(Debug)]
enum HeatmapQueryErrorKind {
    BadFilter(String),
    BadLocator,
    NoSuchSection,
    NoSuchColumn(String),
    MixedUnits(String),
}

/// Indexed refusal or captured-source failure from one Heatmap batch.
#[derive(Debug)]
pub struct HeatmapError {
    ranking_index: usize,
    message: String,
    query_error: Option<HeatmapQueryErrorKind>,
    valid_options: Vec<String>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

pub(crate) struct PreparedHeatmap {
    batch: PreparedHeatmapBatch,
}

pub(crate) struct PreparedHeatmapBatch {
    dataset: Arc<dyn QueryDataset>,
    segments: Vec<DatasetSegment>,
    query: HeatmapBatchQuery,
    unique: Vec<ItemSpec>,
    original_to_unique: Vec<usize>,
    validator_shape: String,
    validator_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemSpec {
    query: HeatmapItemQuery,
    class: ColumnClass,
    unit: Option<Unit>,
    labels: Vec<String>,
    first_index: usize,
}

/// A heatmap request whose complete registry shape has been checked.
#[derive(Clone)]
pub struct ValidatedHeatmapQuery {
    query: HeatmapBatchQuery,
    unique: Vec<ItemSpec>,
    original_to_unique: Vec<usize>,
}

impl std::fmt::Debug for ValidatedHeatmapQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ValidatedHeatmapQuery")
            .field(&self.query)
            .finish()
    }
}

impl PartialEq for ValidatedHeatmapQuery {
    fn eq(&self, other: &Self) -> bool {
        self.query == other.query
    }
}

impl Eq for ValidatedHeatmapQuery {}

struct SharedSectionSpec {
    name: String,
    labels: Vec<String>,
    first_index: usize,
}

pub(crate) fn prepare(
    dataset: Arc<dyn QueryDataset>,
    validated: ValidatedHeatmapQuery,
) -> Result<PreparedHeatmap, QueryError> {
    prepare_batch(dataset, validated)
        .map(|batch| PreparedHeatmap { batch })
        .map_err(HeatmapError::into_query)
}

fn prepare_batch(
    dataset: Arc<dyn QueryDataset>,
    validated: ValidatedHeatmapQuery,
) -> Result<PreparedHeatmapBatch, HeatmapError> {
    let ValidatedHeatmapQuery {
        query,
        unique,
        original_to_unique,
    } = validated;
    let listing = {
        let catalog = dataset
            .catalog()
            .map_err(|error| HeatmapError::storage(0, error))?;
        catalog
            .segments(SegmentSelection::new(SegmentBounds::half_open(
                query.range.from,
                query.range.to_exclusive,
            )))
            .map_err(|error| HeatmapError::storage(0, error))?
    };
    let validator_available = listing.warnings.is_empty();
    let mut segments = listing.segments;
    segments.retain(|segment| {
        segment.max_ts() >= query.range.from && segment.min_ts() < query.range.to_exclusive
    });
    segments.sort_by_key(DatasetSegment::min_ts);
    let validator_shape = format!("summary-v1:{query:?}");
    Ok(PreparedHeatmapBatch {
        dataset,
        segments,
        query,
        unique,
        original_to_unique,
        validator_shape,
        validator_available,
    })
}

pub(crate) fn validate_request(
    query: HeatmapBatchQuery,
) -> Result<ValidatedHeatmapQuery, QueryError> {
    if query.range.from >= query.range.to_exclusive {
        return Err(QueryError::BadFilter("to".to_owned()));
    }
    if query.items.len() != 1 || !matches!(query.items[0].view, HeatmapView::Grid { .. }) {
        return Err(QueryError::BadFilter("heatmap".to_owned()));
    }
    let (unique, original_to_unique) =
        normalize_items(&query.items).map_err(HeatmapError::into_query)?;
    Ok(ValidatedHeatmapQuery {
        query,
        unique,
        original_to_unique,
    })
}

/// Execute one ordered Heatmap batch and return its typed result.
///
/// # Errors
///
/// Returns the expanded ranking index, known replacement names, and the
/// semantic, decoding, cancellation, or captured-source failure.
pub fn execute_heatmap_batch(
    context: &crate::QueryContext,
    query: HeatmapBatchQuery,
    sink: &dyn QuerySink,
) -> Result<HeatmapBatchResult, HeatmapError> {
    if query.items.is_empty() {
        return Err(HeatmapError::invalid(0, "rankings must not be empty"));
    }
    let (unique, original_to_unique) = normalize_items(&query.items)?;
    prepare_batch(
        Arc::clone(&context.dataset),
        ValidatedHeatmapQuery {
            query,
            unique,
            original_to_unique,
        },
    )?
    .execute(sink)
}

impl PreparedHeatmap {
    pub(crate) fn stability(&self) -> QueryStability {
        self.batch.stability()
    }

    pub(crate) fn validator_input(&self) -> Option<(&str, &str, &[DatasetSegment])> {
        self.batch.validator_input()
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let range = self.batch.query.range;
        let result = self.batch.execute(sink).map_err(HeatmapError::into_query)?;
        let Some(item) = result.results.first() else {
            return Ok(());
        };
        stream_grid(item, range.from, range.to_exclusive - 1, sink)
    }
}

impl PreparedHeatmapBatch {
    pub(crate) fn stability(&self) -> QueryStability {
        if self.validator_input().is_some() {
            QueryStability::Immutable
        } else if self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished)
        {
            QueryStability::Revalidate
        } else {
            QueryStability::Mutable
        }
    }

    pub(crate) fn validator_input(&self) -> Option<(&str, &str, &[DatasetSegment])> {
        (self.validator_available
            && !self.segments.is_empty()
            && self
                .segments
                .iter()
                .all(|segment| segment.kind() == SegmentKind::Finished))
        .then_some((
            "heatmap",
            self.validator_shape.as_str(),
            self.segments.as_slice(),
        ))
    }

    fn open_for_scan(&self, segment_ref: &DatasetSegment) -> Result<Segment, HeatmapError> {
        self.dataset
            .open(segment_ref)
            .map_err(|error| HeatmapError::storage(0, error))
    }

    fn open_for_dictionary(
        &self,
        segment_ref: &DatasetSegment,
        ranking_index: usize,
    ) -> Result<Segment, HeatmapError> {
        self.dataset
            .open(segment_ref)
            .map_err(|error| HeatmapError::storage(ranking_index, error))
    }

    pub(crate) fn execute(&self, sink: &dyn QuerySink) -> Result<HeatmapBatchResult, HeatmapError> {
        let (section_specs, accumulator_sections) = shared_section_specs(&self.unique);
        let mut sections = section_specs
            .iter()
            .map(SharedSection::new)
            .collect::<Vec<_>>();
        let mut accumulators = Vec::with_capacity(self.unique.len());
        for (accumulator, spec) in self.unique.iter().enumerate() {
            accumulators.push(Accumulator::new(
                spec,
                self.query.range,
                accumulator_sections[accumulator],
                &section_specs[accumulator_sections[accumulator]],
            ));
        }
        let mut rendered_ids = RenderedIds::new();

        for (segment_slot, segment_ref) in self.segments.iter().enumerate() {
            if sink.cancelled() {
                return Err(HeatmapError::failure(0, "request cancelled"));
            }
            {
                let segment = self.open_for_scan(segment_ref)?;
                let plans = physical_plans(
                    &segment,
                    &self.unique,
                    &section_specs,
                    &accumulator_sections,
                );
                for plan in plans {
                    scan_plan(
                        &segment,
                        segment_slot,
                        &plan,
                        self.query.range,
                        &mut accumulators,
                        &mut sections,
                        sink,
                    )?;
                }
            }
        }

        let mut retained_ids = vec![HashSet::new(); self.segments.len()];
        let mut retained_indices = vec![Vec::new(); self.segments.len()];
        let indexed_sections = sections
            .iter()
            .map(SharedSection::indexed)
            .collect::<Vec<_>>();
        for accumulator in &accumulators {
            accumulator.collect_ids(
                &indexed_sections[accumulator.section],
                &mut retained_ids,
                &mut retained_indices,
            );
        }
        for (segment_slot, ((segment_ref, ids), indexed_ids)) in self
            .segments
            .iter()
            .zip(&retained_ids)
            .zip(&retained_indices)
            .enumerate()
        {
            if ids.is_empty() {
                continue;
            }
            let Some(index) = indexed_ids.first().map(|(_id, index)| *index) else {
                return Err(HeatmapError::failure(
                    0,
                    "retained dictionary IDs have no dependent ranking",
                ));
            };
            {
                let segment = self.open_for_dictionary(segment_ref, index)?;
                let dictionary = segment
                    .dictionary_once_for(ids)
                    .map_err(|error| HeatmapError::storage(index, error))?;
                for (id, index) in indexed_ids {
                    let value = cell(&Cell::StrId(*id), &dictionary)
                        .map_err(|error| HeatmapError::storage(*index, error))?;
                    rendered_ids.insert((segment_slot, *id), value);
                }
            }
        }

        let mut unique_results = Vec::with_capacity(accumulators.len());
        for (spec, accumulator) in self.unique.iter().zip(accumulators) {
            let section = &indexed_sections[accumulator.section];
            unique_results.push(accumulator.finish(spec, &rendered_ids, section)?);
        }
        let results = self
            .original_to_unique
            .iter()
            .map(|index| unique_results[*index].clone())
            .collect();
        Ok(HeatmapBatchResult { results })
    }
}

fn scan_plan(
    segment: &Segment,
    segment_slot: usize,
    plan: &PhysicalPlan,
    range: crate::TimeRange,
    accumulators: &mut [Accumulator],
    sections: &mut [SharedSection],
    sink: &dyn QuerySink,
) -> Result<(), HeatmapError> {
    let take = usize::try_from(plan.rows).unwrap_or(usize::MAX);
    let segment_id = segment.id();
    // Collector statements are classified once per layout, before the scan.
    let collector = if plan.bindings.iter().any(|binding| binding.workload) {
        Some(
            CollectorStatements::scan(segment)
                .map_err(|error| HeatmapError::storage(plan.first_index, error))?,
        )
    } else {
        None
    };
    let mut visited = 0_usize;
    let mut failure = None;
    segment
        .visit_rows(plan.type_id, &plan.projection, 0, take, |ordinal, row| {
            if sink.cancelled() {
                failure = Some(HeatmapError::failure(plan.first_index, "request cancelled"));
                return false;
            }
            visited = visited.saturating_add(1);
            let Some(Cell::Ts(timestamp)) = row.get(plan.timestamp) else {
                return true;
            };
            let timestamp = *timestamp;
            if redundant_cpu_aggregate(plan, &row) {
                return true;
            }
            let collector_row = collector
                .as_ref()
                .is_some_and(|statements| statements.excludes(&row));
            let mut admitted = false;
            for binding in &plan.bindings {
                if collector_row && binding.workload {
                    continue;
                }
                admitted = true;
                let accumulator = &mut accumulators[binding.accumulator];
                if timestamp < range.from {
                    continue;
                }
                if timestamp >= range.to_exclusive {
                    continue;
                }
                accumulator.scan.window_rows = accumulator.scan.window_rows.saturating_add(1);
            }
            if !admitted || timestamp < range.from || timestamp >= range.to_exclusive {
                return true;
            }
            let entity = match sections[plan.section].observe(
                segment_slot,
                segment_id,
                plan.type_id,
                plan.contract,
                &row,
                ordinal,
                timestamp,
                &plan.labels,
            ) {
                Ok(entity) => entity,
                Err(error) => {
                    failure = Some(error);
                    return false;
                }
            };
            for binding in &plan.bindings {
                if collector_row && binding.workload {
                    continue;
                }
                let accumulator = &mut accumulators[binding.accumulator];
                if let Err(error) =
                    accumulator.observe(segment_slot, &row, timestamp, entity, binding)
                {
                    failure = Some(error);
                    return false;
                }
            }
            failure.is_none()
        })
        .map_err(|error| HeatmapError::storage(plan.first_index, error))?;
    if let Some(error) = failure {
        return Err(error);
    }
    debug_assert!(visited <= take, "row visitor exceeded its declared bound");
    Ok(())
}

fn redundant_cpu_aggregate(plan: &PhysicalPlan, row: &Row) -> bool {
    logical_section_name(plan.type_id) == Some("os_cpu")
        && matches!(row.get("cpu_id"), Some(Cell::I32(-1)))
}

#[derive(Default)]
struct ScanStats {
    window_rows: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntityId(usize);

impl EntityId {
    const fn index(self) -> usize {
        self.0
    }
}

struct SharedSection {
    name: String,
    first_index: usize,
    label_count: usize,
    by_raw_key: HashMap<Box<str>, EntityId>,
    entities: Vec<SharedEntity>,
    identity_scratch: Vec<Cell>,
    key_scratch: String,
}

struct SharedEntity {
    type_id: u32,
    identity_segment: usize,
    identity: Box<[Cell]>,
    labels: Box<[Option<StoredLabel>]>,
    locator: StoredLocator,
}

struct StoredLocator {
    segment_slot: usize,
    segment_id: i64,
    timestamp: i64,
    ordinal: u64,
    identity: row_key::RowIdentity,
    event_stream: bool,
}

impl StoredLocator {
    fn observe(
        &mut self,
        segment_slot: usize,
        segment_id: i64,
        timestamp: i64,
        ordinal: u64,
        row: &Row,
    ) -> Result<(), String> {
        let identity = row_key::identity(row.contract().type_id.get(), row)?;
        if (timestamp, segment_slot) == (self.timestamp, self.segment_slot)
            && ordinal != self.ordinal
            && identity == self.identity
            && !self.event_stream
        {
            return Err(format!(
                "cannot emit detail_locator: type_id {} has a non-unique identity at timestamp {timestamp}",
                row.contract().type_id.get(),
            ));
        }
        if (timestamp, segment_slot, ordinal) < (self.timestamp, self.segment_slot, self.ordinal) {
            return Ok(());
        }
        self.segment_slot = segment_slot;
        self.segment_id = segment_id;
        self.timestamp = timestamp;
        self.ordinal = ordinal;
        self.identity = identity;
        Ok(())
    }
}

struct IndexedSection<'a> {
    entities: &'a [SharedEntity],
    raw_keys: Vec<&'a str>,
}

impl SharedSection {
    fn new(spec: &SharedSectionSpec) -> Self {
        Self {
            name: spec.name.clone(),
            first_index: spec.first_index,
            label_count: spec.labels.len(),
            by_raw_key: HashMap::new(),
            entities: Vec::new(),
            identity_scratch: Vec::new(),
            key_scratch: String::new(),
        }
    }

    fn indexed(&self) -> IndexedSection<'_> {
        let mut raw_keys = vec![""; self.entities.len()];
        for (raw_key, entity) in &self.by_raw_key {
            raw_keys[entity.index()] = raw_key;
        }
        IndexedSection {
            entities: &self.entities,
            raw_keys,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the shared row identity and latest-label observation"
    )]
    fn observe(
        &mut self,
        segment_slot: usize,
        segment_id: i64,
        type_id: u32,
        contract: &'static kronika_registry::TypeContract,
        row: &Row,
        ordinal: u64,
        timestamp: i64,
        labels: &[Option<&'static str>],
    ) -> Result<EntityId, HeatmapError> {
        self.identity_scratch.clear();
        self.identity_scratch.extend(
            contract
                .identity
                .iter()
                .map(|name| row.get(name).cloned().unwrap_or(Cell::Null)),
        );
        raw_key_into(&mut self.key_scratch, type_id, &self.identity_scratch);
        let entity = if let Some(entity) = self.by_raw_key.get(self.key_scratch.as_str()).copied() {
            self.entities[entity.index()]
                .locator
                .observe(segment_slot, segment_id, timestamp, ordinal, row)
                .map_err(|error| HeatmapError::bad_locator(self.first_index, error))?;
            entity
        } else {
            let raw_key: Box<str> = self.key_scratch.clone().into_boxed_str();
            u32::try_from(self.entities.len()).map_err(|_error| {
                HeatmapError::invalid(
                    self.first_index,
                    format!(
                        "retained {} {} identities before ranking; the entity cardinality cannot be represented",
                        self.entities.len(), self.name
                    ),
                )
            })?;
            let entity = EntityId(self.entities.len());
            self.by_raw_key.insert(raw_key, entity);
            let locator_identity = row_key::identity(row.contract().type_id.get(), row)
                .map_err(|error| HeatmapError::bad_locator(self.first_index, error))?;
            self.entities.push(SharedEntity {
                type_id,
                identity_segment: segment_slot,
                identity: self.identity_scratch.clone().into_boxed_slice(),
                labels: vec![None; self.label_count].into_boxed_slice(),
                locator: StoredLocator {
                    segment_slot,
                    segment_id,
                    timestamp,
                    ordinal,
                    identity: locator_identity,
                    event_stream: contract.semantics == kronika_registry::Semantics::EventStream,
                },
            });
            entity
        };
        let shared = &mut self.entities[entity.index()];
        for (slot, column) in shared.labels.iter_mut().zip(labels) {
            let Some(value) = column.and_then(|name| row.get(name)) else {
                continue;
            };
            if matches!(value, Cell::Null) {
                continue;
            }
            let replace = slot.as_ref().is_none_or(|stored| {
                (timestamp, segment_slot, ordinal)
                    >= (stored.timestamp, stored.segment_slot, stored.ordinal)
            });
            if replace {
                *slot = Some(StoredLabel {
                    segment_slot,
                    timestamp,
                    ordinal,
                    value: value.clone(),
                });
            }
        }
        Ok(entity)
    }
}

fn reserve_ids(
    cells: &[Cell],
    segment_slot: usize,
    retained: &mut [HashSet<u64>],
    retained_indices: &mut [Vec<(u64, usize)>],
    index: usize,
) {
    for stored in cells {
        reserve_id(stored, segment_slot, retained, retained_indices, index);
    }
}

fn reserve_id(
    stored: &Cell,
    segment_slot: usize,
    retained: &mut [HashSet<u64>],
    retained_indices: &mut [Vec<(u64, usize)>],
    index: usize,
) {
    if let Cell::StrId(id) = stored
        && retained[segment_slot].insert(*id)
    {
        retained_indices[segment_slot].push((*id, index));
    }
}

fn raw_key_into(key: &mut String, type_id: u32, cells: &[Cell]) {
    use std::fmt::Write as _;
    key.clear();
    let _ = write!(key, "{type_id}:");
    for cell in cells {
        match cell {
            Cell::I16(value) => {
                let _ = write!(key, "a{value};");
            }
            Cell::I32(value) => {
                let _ = write!(key, "b{value};");
            }
            Cell::I64(value) => {
                let _ = write!(key, "c{value};");
            }
            Cell::U32(value) => {
                let _ = write!(key, "d{value};");
            }
            Cell::U64(value) => {
                let _ = write!(key, "e{value};");
            }
            Cell::F64(value) => {
                let _ = write!(key, "f{:016x};", value.to_bits());
            }
            Cell::Bool(value) => {
                let _ = write!(key, "g{};", u8::from(*value));
            }
            Cell::Ts(value) => {
                let _ = write!(key, "h{value};");
            }
            Cell::StrId(value) => {
                let _ = write!(key, "i{value:016x};");
            }
            Cell::ListI32(values) => {
                let _ = write!(key, "j{}:", values.len());
                for value in values {
                    let _ = write!(key, "{value},");
                }
                key.push(';');
            }
            Cell::Null => key.push_str("n;"),
        }
    }
}

pub(super) fn summed(row: &Row, columns: &[&str]) -> Option<f64> {
    let mut sum = None;
    for column in columns {
        if let Some(value) = row.get(column).and_then(numeric) {
            sum = Some(sum.unwrap_or(0.0) + value);
        }
    }
    sum
}

fn numeric(stored: &Cell) -> Option<f64> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "counters below 2^53 are exact; floating-point division makes rates approximate"
    )]
    match stored {
        Cell::I16(value) => Some(f64::from(*value)),
        Cell::I32(value) => Some(f64::from(*value)),
        Cell::I64(value) | Cell::Ts(value) => Some(*value as f64),
        Cell::U32(value) => Some(f64::from(*value)),
        Cell::U64(value) => Some(*value as f64),
        Cell::F64(value) => value.is_finite().then_some(*value),
        Cell::Bool(_) | Cell::StrId(_) | Cell::ListI32(_) | Cell::Null => None,
    }
}

pub(super) fn entity_key_into(key: &mut String, type_id: u32, identity: &[Value]) {
    use std::fmt::Write as _;
    key.clear();
    let _ = write!(key, "{type_id}");
    for value in identity {
        key.push('\u{1f}');
        match value {
            Value::String(text) => key.push_str(text),
            Value::Null => key.push('\u{0}'),
            other => {
                let _ = write!(key, "{other}");
            }
        }
    }
}

struct PhysicalPlan {
    section: usize,
    type_id: u32,
    contract: &'static kronika_registry::TypeContract,
    rows: u64,
    timestamp: &'static str,
    projection: Vec<&'static str>,
    labels: Vec<Option<&'static str>>,
    bindings: Vec<Binding>,
    first_index: usize,
}

struct Binding {
    accumulator: usize,
    metrics: Vec<&'static str>,
    groups: Vec<Option<&'static str>>,
    /// This ranking excludes the collector's own statements.
    workload: bool,
}

struct Accumulator {
    section: usize,
    label_slots: Vec<usize>,
    range: crate::TimeRange,
    columns: usize,
    cumulative: bool,
    rss_mean: Option<RssMean>,
    grid: bool,
    grouped: bool,
    top: usize,
    first_index: usize,
    folds: FoldArena,
    totals: Vec<CellSum>,
    groups: Vec<GroupState>,
    group_index: HashMap<String, usize>,
    out_of_order: u64,
    scan: ScanStats,
}

#[derive(Default)]
struct RssMean {
    timestamps: HashSet<i64>,
    sums: Vec<f64>,
}

struct RankFold {
    entity: EntityId,
    window: Obs,
}

struct GridFold {
    entity: EntityId,
    window: Obs,
    column: usize,
    current: Obs,
    carry: Option<(i64, f64)>,
    cells: Vec<Obs>,
    grid_carry: Option<(i64, f64)>,
    group: Option<usize>,
}

enum FoldArena {
    Ranking {
        slot_by_entity: Vec<u32>,
        folds: Vec<RankFold>,
    },
    Grid {
        slot_by_entity: Vec<u32>,
        folds: Vec<GridFold>,
    },
}

#[derive(Clone)]
struct StoredLabel {
    segment_slot: usize,
    timestamp: i64,
    ordinal: u64,
    value: Cell,
}

struct GroupState {
    segment_slot: usize,
    values: Vec<Cell>,
    members: u32,
}

#[derive(Clone, Copy)]
enum LabelCutoff {
    Value(f64),
    Null,
}

struct RankedState {
    key: String,
    entity: EntityId,
    total: Option<f64>,
    identity_values: Vec<Value>,
    grid: Option<GridFold>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Obs {
    pub(super) count: u32,
    first_ts: i64,
    first_value: f64,
    pub(super) last_ts: i64,
    pub(super) last_value: f64,
    max_value: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct CellSum {
    sum: f64,
    contributors: u32,
}

#[cfg(test)]
#[path = "../tests/heatmap_execution.rs"]
mod tests;
