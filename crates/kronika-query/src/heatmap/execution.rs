//! One-pass Heatmap planning, execution, and transport-independent folding.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use kronika_reader::{Cell, Dictionary, Row, Segment, SegmentKind};
use kronika_registry::{ColumnClass, Unit, contract, logical_section_name, registry};
use serde_json::{Value, json};

use super::query::{HeatmapBatchQuery, HeatmapItemQuery, HeatmapView, MAX_FIELDS, MAX_TOP};
use super::result::{
    CoverageState, HeatmapBand, HeatmapBatchResult, HeatmapCoverage, HeatmapEntity, HeatmapGrid,
    HeatmapGroup, HeatmapInterval, HeatmapItemResult, HeatmapSummary, NamedValues,
};
use crate::STATEMENTS_SECTION;
use crate::render::{cell, record};
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

impl HeatmapError {
    fn invalid(ranking_index: usize, message: impl Into<String>) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadFilter("heatmap".to_owned()),
            Vec::new(),
        )
    }

    fn invalid_parameter(
        ranking_index: usize,
        message: impl Into<String>,
        parameter: &str,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadFilter(parameter.to_owned()),
            Vec::new(),
        )
    }

    fn bad_locator(ranking_index: usize, message: impl Into<String>) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::BadLocator,
            Vec::new(),
        )
    }

    fn no_such_section(
        ranking_index: usize,
        message: impl Into<String>,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::NoSuchSection,
            valid_options,
        )
    }

    fn no_such_column(
        ranking_index: usize,
        message: impl Into<String>,
        column: String,
        valid_options: Vec<String>,
    ) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::NoSuchColumn(column),
            valid_options,
        )
    }

    fn mixed_units(ranking_index: usize, message: impl Into<String>, fields: String) -> Self {
        Self::invalid_as(
            ranking_index,
            message,
            HeatmapQueryErrorKind::MixedUnits(fields),
            Vec::new(),
        )
    }

    fn invalid_as(
        ranking_index: usize,
        message: impl Into<String>,
        query_error: HeatmapQueryErrorKind,
        valid_options: Vec<String>,
    ) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            query_error: Some(query_error),
            valid_options,
            source: None,
        }
    }

    fn storage<E>(ranking_index: usize, error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self {
            ranking_index,
            message: error.to_string(),
            query_error: None,
            valid_options: Vec::new(),
            source: Some(Box::new(error)),
        }
    }

    fn failure(ranking_index: usize, message: impl Into<String>) -> Self {
        Self {
            ranking_index,
            message: message.into(),
            query_error: None,
            valid_options: Vec::new(),
            source: None,
        }
    }

    /// Zero-based expanded-item index that refused the batch.
    #[must_use]
    pub const fn ranking_index(&self) -> usize {
        self.ranking_index
    }

    /// Registry names valid in place of an unknown section or field.
    #[must_use]
    pub fn valid_options(&self) -> &[String] {
        &self.valid_options
    }

    /// Reduce this indexed batch error to the common query error family.
    #[must_use]
    pub fn into_query(mut self) -> QueryError {
        match self.query_error.take() {
            Some(HeatmapQueryErrorKind::BadFilter(parameter)) => QueryError::BadFilter(parameter),
            Some(HeatmapQueryErrorKind::BadLocator) => QueryError::BadLocator(self.message),
            Some(HeatmapQueryErrorKind::NoSuchSection) => QueryError::NoSuchSection,
            Some(HeatmapQueryErrorKind::NoSuchColumn(column)) => QueryError::NoSuchColumn(column),
            Some(HeatmapQueryErrorKind::MixedUnits(fields)) => QueryError::MixedUnits(fields),
            None => QueryError::Unreadable(Box::new(self)),
        }
    }
}

impl std::fmt::Display for HeatmapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rankings[{}]: {}", self.ranking_index, self.message)
    }
}

impl std::error::Error for HeatmapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let source: &(dyn std::error::Error + 'static) = self.source.as_deref()?;
        Some(source)
    }
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

fn normalize_items(
    items: &[HeatmapItemQuery],
) -> Result<(Vec<ItemSpec>, Vec<usize>), HeatmapError> {
    let mut unique = Vec::new();
    let mut positions = HashMap::new();
    let mut original_to_unique = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(position) = positions.get(item).copied() {
            original_to_unique.push(position);
            continue;
        }
        let (class, unit, labels) = validate_item(item, index)?;
        let position = unique.len();
        positions.insert(item.clone(), position);
        original_to_unique.push(position);
        unique.push(ItemSpec {
            query: item.clone(),
            class,
            unit,
            labels,
            first_index: index,
        });
    }
    Ok((unique, original_to_unique))
}

fn shared_section_specs(specs: &[ItemSpec]) -> (Vec<SharedSectionSpec>, Vec<usize>) {
    let mut sections = Vec::<SharedSectionSpec>::new();
    let mut positions = HashMap::<&str, usize>::new();
    let mut accumulator_sections = Vec::with_capacity(specs.len());
    for spec in specs {
        let section = spec.query.ranking.section.as_str();
        let position = positions.get(section).copied().unwrap_or_else(|| {
            let position = sections.len();
            positions.insert(section, position);
            sections.push(SharedSectionSpec {
                name: section.to_owned(),
                labels: Vec::new(),
                first_index: spec.first_index,
            });
            position
        });
        if spec.query.view.groups().is_empty() {
            for label in &spec.labels {
                if !sections[position].labels.contains(label) {
                    sections[position].labels.push(label.clone());
                }
            }
        }
        accumulator_sections.push(position);
    }
    (sections, accumulator_sections)
}

#[expect(
    clippy::too_many_lines,
    reason = "validation keeps every indexed ranking error in request order"
)]
fn validate_item(
    item: &HeatmapItemQuery,
    index: usize,
) -> Result<(ColumnClass, Option<Unit>, Vec<String>), HeatmapError> {
    let ranking = &item.ranking;
    if ranking.section.is_empty() || ranking.section.len() > 128 {
        return Err(HeatmapError::invalid_parameter(
            index,
            "section must contain 1 to 128 UTF-8 bytes",
            "section",
        ));
    }
    match item.view {
        HeatmapView::RankingOnly if ranking.fields.len() != 1 => {
            return Err(HeatmapError::invalid_parameter(
                index,
                "an Overview result must contain exactly one field",
                "field",
            ));
        }
        HeatmapView::Grid { .. } if !(1..=MAX_FIELDS).contains(&ranking.fields.len()) => {
            return Err(HeatmapError::invalid_parameter(
                index,
                format!("fields must contain 1 to {MAX_FIELDS} names"),
                "field",
            ));
        }
        HeatmapView::RankingOnly | HeatmapView::Grid { .. } => {}
    }
    let mut seen = HashSet::new();
    for field in &ranking.fields {
        if field.is_empty() || !seen.insert(field) {
            return Err(HeatmapError::invalid_parameter(
                index,
                format!("field {field:?} is empty or repeated"),
                "field",
            ));
        }
    }
    if !(1..=MAX_TOP).contains(&ranking.top) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("top must be between 1 and {MAX_TOP}, got {}", ranking.top),
            "top",
        ));
    }
    if !item.scope.allows_rows(&ranking.section) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("scope=workload applies only to {STATEMENTS_SECTION}"),
            "scope",
        ));
    }

    let wanted_type = item.view.type_id();
    let contracts: Vec<_> = registry()
        .iter()
        .filter(|contract| {
            logical_section_name(contract.type_id.get()) == Some(ranking.section.as_str())
                && wanted_type.is_none_or(|wanted| wanted == contract.type_id.get())
        })
        .collect();
    if contracts.is_empty() {
        let mut seen = HashSet::new();
        let valid_options = registry()
            .iter()
            .filter_map(|contract| logical_section_name(contract.type_id.get()))
            .filter(|section| seen.insert(*section))
            .map(str::to_owned)
            .collect();
        return Err(HeatmapError::no_such_section(
            index,
            "no such logical section",
            valid_options,
        ));
    }
    let numeric_options = || {
        let mut seen = HashSet::new();
        contracts
            .iter()
            .flat_map(|contract| contract.columns)
            .filter(|column| {
                matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
                    && seen.insert(column.name)
            })
            .map(|column| column.name.to_owned())
            .collect::<Vec<_>>()
    };
    let mut class = None;
    let mut unit = None;
    for field in &ranking.fields {
        let columns: Vec<_> = contracts
            .iter()
            .filter_map(|contract| contract.column(field))
            .collect();
        if columns.is_empty() {
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {field:?}"),
                field.clone(),
                numeric_options(),
            ));
        }
        for column in columns {
            if !matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge) {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!("column {field:?} is not numeric"),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if class
                .replace(column.class)
                .is_some_and(|seen| seen != column.class)
            {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!(
                        "fields carry different classes: {}",
                        ranking.fields.join("+")
                    ),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if unit
                .replace(column.unit)
                .is_some_and(|seen| seen != column.unit)
            {
                let fields = ranking.fields.join("+");
                return Err(HeatmapError::mixed_units(
                    index,
                    format!("fields carry different units: {fields}"),
                    fields,
                ));
            }
        }
    }
    for group in item.view.groups() {
        if group.is_empty()
            || !contracts
                .iter()
                .any(|contract| contract.column(group).is_some())
        {
            let mut seen = HashSet::new();
            let valid_options = contracts
                .iter()
                .flat_map(|contract| contract.columns)
                .filter(|column| seen.insert(column.name))
                .map(|column| column.name.to_owned())
                .collect();
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {group:?}"),
                group.clone(),
                valid_options,
            ));
        }
    }
    let mut labels = Vec::new();
    let mut label_seen = HashSet::new();
    for contract in contracts {
        for column in contract.columns {
            if column.class == ColumnClass::Label
                && !row_key::is_detail_text(&ranking.section, column.name)
                && label_seen.insert(column.name)
            {
                labels.push(column.name.to_owned());
            }
        }
    }
    let class = class.ok_or_else(|| {
        HeatmapError::invalid(index, "numeric field validation found no quantity class")
    })?;
    Ok((class, unit.unwrap_or(None), labels))
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

fn physical_plans(
    segment: &Segment,
    specs: &[ItemSpec],
    sections: &[SharedSectionSpec],
    accumulator_sections: &[usize],
) -> Vec<PhysicalPlan> {
    let mut plans = Vec::new();
    for (section, shared) in sections.iter().enumerate() {
        for (type_id, stored) in segment.layouts(&shared.name) {
            let Some(contract) = contract(type_id) else {
                continue;
            };
            let Some(timestamp) = contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
            else {
                continue;
            };
            let mut projection = vec![timestamp];
            projection.extend(row_key::identity_columns(contract));
            let labels = shared
                .labels
                .iter()
                .map(|name| contract.column(name).map(|column| column.name))
                .collect::<Vec<_>>();
            projection.extend(labels.iter().flatten().copied());
            let mut bindings = Vec::new();
            let mut first_index = usize::MAX;
            for (accumulator, spec) in specs.iter().enumerate() {
                if accumulator_sections[accumulator] != section
                    || spec
                        .query
                        .view
                        .type_id()
                        .is_some_and(|wanted| wanted != type_id)
                {
                    continue;
                }
                let metrics: Vec<_> = spec
                    .query
                    .ranking
                    .fields
                    .iter()
                    .filter_map(|name| contract.column(name).map(|column| column.name))
                    .collect();
                if metrics.is_empty() {
                    continue;
                }
                let groups = spec
                    .query
                    .view
                    .groups()
                    .iter()
                    .map(|name| contract.column(name).map(|column| column.name))
                    .collect::<Vec<_>>();
                projection.extend(metrics.iter().copied());
                projection.extend(groups.iter().flatten().copied());
                let scope_column = spec
                    .query
                    .scope
                    .filters(&shared.name)
                    .then(|| contract.column("query").map(|column| column.name))
                    .flatten();
                projection.extend(scope_column);
                bindings.push(Binding {
                    accumulator,
                    metrics,
                    groups,
                    workload: scope_column.is_some(),
                });
                first_index = first_index.min(spec.first_index);
            }
            if bindings.is_empty() {
                continue;
            }
            projection.sort_unstable();
            projection.dedup();
            plans.push(PhysicalPlan {
                section,
                type_id,
                contract,
                rows: stored.rows,
                timestamp,
                projection,
                labels,
                bindings,
                first_index,
            });
        }
    }
    plans
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

impl RssMean {
    fn observe(&mut self, entity: EntityId, timestamp: i64, value: f64) {
        self.timestamps.insert(timestamp);
        let index = entity.index();
        if self.sums.len() <= index {
            self.sums.resize(index + 1, 0.0);
        }
        self.sums[index] += value;
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "a query cannot retain 2^53 recorded snapshot timestamps"
    )]
    fn mean(&self, entity: EntityId) -> Option<f64> {
        if self.timestamps.is_empty() {
            return None;
        }
        self.sums
            .get(entity.index())
            .map(|sum| sum / self.timestamps.len() as f64)
    }
}

const NO_FOLD: u32 = u32::MAX;

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

impl Accumulator {
    fn new(
        spec: &ItemSpec,
        range: crate::TimeRange,
        section: usize,
        shared: &SharedSectionSpec,
    ) -> Self {
        let columns = spec.query.view.columns();
        let grid = matches!(spec.query.view, HeatmapView::Grid { .. });
        let grouped = !spec.query.view.groups().is_empty();
        let label_slots = if grouped {
            Vec::new()
        } else {
            spec.labels
                .iter()
                .filter_map(|name| shared.labels.iter().position(|candidate| candidate == name))
                .collect()
        };
        Self {
            section,
            label_slots,
            range,
            columns,
            cumulative: spec.class == ColumnClass::Cumulative,
            rss_mean: (grid
                && spec.query.ranking.section == "os_process"
                && spec.query.ranking.fields == ["rmem_kb"])
            .then(RssMean::default),
            grid,
            grouped,
            top: spec.query.ranking.top,
            first_index: spec.first_index,
            folds: if grid {
                FoldArena::Grid {
                    slot_by_entity: Vec::new(),
                    folds: Vec::new(),
                }
            } else {
                FoldArena::Ranking {
                    slot_by_entity: Vec::new(),
                    folds: Vec::new(),
                }
            },
            totals: if grid {
                vec![CellSum::default(); columns]
            } else {
                Vec::new()
            },
            groups: Vec::new(),
            group_index: HashMap::new(),
            out_of_order: 0,
            scan: ScanStats::default(),
        }
    }

    fn observe(
        &mut self,
        segment_slot: usize,
        row: &Row,
        timestamp: i64,
        entity: EntityId,
        binding: &Binding,
    ) -> Result<(), HeatmapError> {
        let Some(value) = summed(row, &binding.metrics) else {
            return Ok(());
        };
        if let Some(mean) = &mut self.rss_mean {
            mean.observe(entity, timestamp, value);
        }
        if !self.grid {
            self.rank_fold(entity)?.window.observe(timestamp, value);
            return Ok(());
        }

        let (fold, inserted) = self.grid_fold(entity, timestamp)?;
        let group = (inserted && self.grouped).then(|| {
            let values: Vec<Cell> = binding
                .groups
                .iter()
                .map(|column| {
                    column
                        .and_then(|name| row.get(name))
                        .cloned()
                        .unwrap_or(Cell::Null)
                })
                .collect();
            let mut group_key = String::new();
            raw_key_into(&mut group_key, 0, &values);
            let group = if let Some(group) = self.group_index.get(&group_key).copied() {
                group
            } else {
                self.groups.push(GroupState {
                    segment_slot,
                    values,
                    members: 0,
                });
                let group = self.groups.len() - 1;
                self.group_index.insert(group_key, group);
                group
            };
            self.groups[group].members = self.groups[group].members.saturating_add(1);
            group
        });
        let FoldArena::Grid { folds, .. } = &mut self.folds else {
            return Err(HeatmapError::invalid(
                self.first_index,
                "grid observation reached a non-grid accumulator",
            ));
        };
        let state = &mut folds[fold];
        if inserted {
            state.group = group;
        }

        let previous_ts = (state.window.count > 0).then_some(state.window.last_ts);
        let column = column_of_span(previous_ts, timestamp, self.range, self.columns);
        state.window.observe(timestamp, value);
        if self.grid && (!self.grouped || column >= state.column) {
            let grid_column = column_of_span(
                state.grid_carry.map(|(carry_ts, _value)| carry_ts),
                timestamp,
                self.range,
                self.columns,
            );
            if self.cumulative
                && state.cells[grid_column].count == 0
                && let Some((carry_ts, carry_value)) = state.grid_carry
                && carry_ts < timestamp
            {
                state.cells[grid_column].observe(carry_ts, carry_value);
            }
            state.cells[grid_column].observe(timestamp, value);
            if state
                .grid_carry
                .is_none_or(|(carry_ts, _value)| carry_ts <= timestamp)
            {
                state.grid_carry = Some((timestamp, value));
            }
        }
        if column < state.column {
            self.out_of_order = self.out_of_order.saturating_add(1);
            return Ok(());
        }
        if column > state.column {
            if self.grid
                && let Some(finished) = state.current.cell(self.cumulative)
            {
                self.totals[state.column].add(finished);
            }
            if state.current.count > 0 {
                state.carry = Some((state.current.last_ts, state.current.last_value));
            }
            state.column = column;
            state.current = Obs::default();
            if self.cumulative
                && let Some((carry_ts, carry_value)) = state.carry
            {
                state.current.observe(carry_ts, carry_value);
            }
        }
        state.current.observe(timestamp, value);
        Ok(())
    }

    fn rank_fold(&mut self, entity: EntityId) -> Result<&mut RankFold, HeatmapError> {
        let FoldArena::Ranking {
            slot_by_entity,
            folds,
        } = &mut self.folds
        else {
            return Err(HeatmapError::invalid(
                self.first_index,
                "ranking observation reached a grid accumulator",
            ));
        };
        let entity_index = entity.index();
        if slot_by_entity.len() <= entity_index {
            slot_by_entity.resize(entity_index + 1, NO_FOLD);
        }
        if slot_by_entity[entity_index] == NO_FOLD {
            let slot = u32::try_from(folds.len())
                .ok()
                .filter(|slot| *slot != NO_FOLD)
                .ok_or_else(|| {
                    HeatmapError::invalid(
                        self.first_index,
                        "metric fold cardinality cannot be represented",
                    )
                })?;
            folds.push(RankFold {
                entity,
                window: Obs::default(),
            });
            slot_by_entity[entity_index] = slot;
        }
        let slot = usize::try_from(slot_by_entity[entity_index]).map_err(|_error| {
            HeatmapError::invalid(self.first_index, "metric fold slot does not fit usize")
        })?;
        Ok(&mut folds[slot])
    }

    fn grid_fold(
        &mut self,
        entity: EntityId,
        timestamp: i64,
    ) -> Result<(usize, bool), HeatmapError> {
        let FoldArena::Grid {
            slot_by_entity,
            folds,
        } = &mut self.folds
        else {
            return Err(HeatmapError::invalid(
                self.first_index,
                "grid observation reached a ranking accumulator",
            ));
        };
        let entity_index = entity.index();
        if slot_by_entity.len() <= entity_index {
            slot_by_entity.resize(entity_index + 1, NO_FOLD);
        }
        if slot_by_entity[entity_index] != NO_FOLD {
            let slot = usize::try_from(slot_by_entity[entity_index]).map_err(|_error| {
                HeatmapError::invalid(self.first_index, "metric fold slot does not fit usize")
            })?;
            return Ok((slot, false));
        }
        let slot = u32::try_from(folds.len())
            .ok()
            .filter(|slot| *slot != NO_FOLD)
            .ok_or_else(|| {
                HeatmapError::invalid(
                    self.first_index,
                    "metric fold cardinality cannot be represented",
                )
            })?;
        folds.push(GridFold {
            entity,
            window: Obs::default(),
            column: column_of(timestamp, self.range, self.columns),
            current: Obs::default(),
            carry: None,
            cells: vec![Obs::default(); self.columns],
            grid_carry: None,
            group: None,
        });
        slot_by_entity[entity_index] = slot;
        let slot = usize::try_from(slot).map_err(|_error| {
            HeatmapError::invalid(self.first_index, "metric fold slot does not fit usize")
        })?;
        Ok((slot, true))
    }

    fn collect_ids(
        &self,
        section: &IndexedSection<'_>,
        retained: &mut [HashSet<u64>],
        retained_indices: &mut [Vec<(u64, usize)>],
    ) {
        if self.grouped {
            let group_totals = self.group_totals(section);
            let order = group_order(&group_totals);
            for group in order.into_iter().take(self.top) {
                let group = &self.groups[group];
                reserve_ids(
                    &group.values,
                    group.segment_slot,
                    retained,
                    retained_indices,
                    self.first_index,
                );
            }
            return;
        }
        let label_cutoff = self.label_cutoff();
        self.for_each_fold(|entity, window| {
            let state = &section.entities[entity.index()];
            reserve_ids(
                &state.identity,
                state.identity_segment,
                retained,
                retained_indices,
                self.first_index,
            );
            if label_cutoff
                .is_some_and(|cutoff| ranking_reaches_cutoff(self.score(entity, window), cutoff))
            {
                for slot in &self.label_slots {
                    if let Some(label) = &state.labels[*slot] {
                        reserve_id(
                            &label.value,
                            label.segment_slot,
                            retained,
                            retained_indices,
                            self.first_index,
                        );
                    }
                }
            }
        });
    }

    fn group_totals(&self, section: &IndexedSection<'_>) -> Vec<Option<f64>> {
        let mut totals = vec![None; self.groups.len()];
        for state in self.ordered_grid_folds(section) {
            if let (Some(group), Some(total)) =
                (state.group, self.score(state.entity, &state.window))
            {
                totals[group] = Some(totals[group].unwrap_or(0.0) + total);
            }
        }
        totals
    }

    fn ordered_grid_folds<'a>(&'a self, section: &IndexedSection<'_>) -> Vec<&'a GridFold> {
        let FoldArena::Grid { folds, .. } = &self.folds else {
            return Vec::new();
        };
        let mut states = folds.iter().collect::<Vec<_>>();
        states.sort_unstable_by(|left, right| {
            compare_totals(
                self.score(left.entity, &left.window).as_ref(),
                self.score(right.entity, &right.window).as_ref(),
            )
            .then_with(|| {
                section.raw_keys[left.entity.index()].cmp(section.raw_keys[right.entity.index()])
            })
        });
        states
    }

    const fn fold_count(&self) -> usize {
        match &self.folds {
            FoldArena::Ranking { folds, .. } => folds.len(),
            FoldArena::Grid { folds, .. } => folds.len(),
        }
    }

    fn for_each_fold(&self, mut visit: impl FnMut(EntityId, &Obs)) {
        match &self.folds {
            FoldArena::Ranking { folds, .. } => {
                for fold in folds {
                    visit(fold.entity, &fold.window);
                }
            }
            FoldArena::Grid { folds, .. } => {
                for fold in folds {
                    visit(fold.entity, &fold.window);
                }
            }
        }
    }

    fn label_cutoff(&self) -> Option<LabelCutoff> {
        if self.grouped {
            return None;
        }
        let mut totals = Vec::with_capacity(self.fold_count());
        self.for_each_fold(|entity, window| totals.push(self.score(entity, window)));
        totals.sort_by(|left, right| compare_totals(left.as_ref(), right.as_ref()));
        totals
            .get(self.top.min(totals.len()).saturating_sub(1))
            .copied()
            .map(|total| total.map_or(LabelCutoff::Null, LabelCutoff::Value))
    }

    fn score(&self, entity: EntityId, window: &Obs) -> Option<f64> {
        self.rss_mean
            .as_ref()
            .map_or_else(|| window.total(self.cumulative), |mean| mean.mean(entity))
    }

    const fn summary(&self) -> HeatmapSummary {
        if self.rss_mean.is_some() {
            HeatmapSummary::Mean
        } else if self.cumulative {
            HeatmapSummary::Sum
        } else {
            HeatmapSummary::Max
        }
    }

    const fn additive_summary(&self) -> bool {
        self.cumulative || self.rss_mean.is_some()
    }

    fn finish(
        mut self,
        spec: &ItemSpec,
        dictionary: &RenderedIds,
        section: &IndexedSection<'_>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let has_data = self.fold_count() > 0;
        let coverage = HeatmapCoverage {
            state: if has_data {
                CoverageState::Data
            } else {
                CoverageState::NoData
            },
            window_rows: self.scan.window_rows,
        };
        let ranking = spec.query.ranking.clone();
        if self.grouped {
            return self.finish_grouped(spec, coverage, ranking, dictionary, section);
        }
        let mut ranked = Vec::new();
        let folds = std::mem::replace(
            &mut self.folds,
            FoldArena::Ranking {
                slot_by_entity: Vec::new(),
                folds: Vec::new(),
            },
        );
        let rows = match folds {
            FoldArena::Ranking { folds, .. } => folds
                .into_iter()
                .map(|fold| (fold.entity, fold.window, None))
                .collect::<Vec<_>>(),
            FoldArena::Grid { folds, .. } => folds
                .into_iter()
                .map(|fold| {
                    if let Some(finished) = fold.current.cell(self.cumulative) {
                        self.totals[fold.column].add(finished);
                    }
                    let window = fold.window;
                    (fold.entity, window, Some(fold))
                })
                .collect::<Vec<_>>(),
        };
        for (entity, window, grid) in rows {
            let state = &section.entities[entity.index()];
            let identity_values = render_cells(
                &state.identity,
                state.identity_segment,
                dictionary,
                self.first_index,
            )?;
            let mut key = String::new();
            entity_key_into(&mut key, state.type_id, &identity_values);
            ranked.push(RankedState {
                key,
                entity,
                total: self.score(entity, &window),
                identity_values,
                grid,
            });
        }
        ranked.sort_by(|left, right| {
            compare_totals(left.total.as_ref(), right.total.as_ref())
                .then_with(|| left.key.cmp(&right.key))
        });
        let physical_entity_count = u64::try_from(ranked.len()).unwrap_or(u64::MAX);
        let totals_total = summary_total(
            ranked.iter().filter_map(|row| row.total),
            self.additive_summary(),
        );
        self.finish_ungrouped(
            spec,
            ranked,
            physical_entity_count,
            totals_total,
            coverage,
            ranking,
            dictionary,
            section,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the typed result's shared completed parts"
    )]
    fn finish_ungrouped(
        self,
        spec: &ItemSpec,
        mut ranked: Vec<RankedState>,
        entity_count: u64,
        totals_total: Option<f64>,
        coverage: HeatmapCoverage,
        ranking: super::query::NormalizedRanking,
        dictionary: &RenderedIds,
        section: &IndexedSection<'_>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let top = spec.query.ranking.top;
        let summary = self.summary();
        let additive_summary = self.additive_summary();
        let others_total = summary_total(
            ranked.iter().skip(top).filter_map(|row| row.total),
            additive_summary,
        );
        let totals = self.totals;
        ranked.truncate(top);
        let mut winner_sums = vec![CellSum::default(); self.columns];
        let mut entities = Vec::with_capacity(ranked.len());
        for row in ranked {
            if let Some(grid) = &row.grid {
                for (sum, observed) in winner_sums.iter_mut().zip(&grid.cells) {
                    if let Some(value) = observed.cell(self.cumulative) {
                        sum.add(value);
                    }
                }
            }
            let shared = &section.entities[row.entity.index()];
            let labels = labels_object(
                spec,
                &shared.labels,
                &self.label_slots,
                dictionary,
                self.first_index,
            )?;
            entities.push(HeatmapEntity {
                identity: identity_object(shared.type_id, row.identity_values),
                labels,
                detail_locator: row_key::detail_locator(
                    &spec.query.ranking.section,
                    shared.locator.segment_id,
                    shared.locator.timestamp,
                    shared.type_id,
                    shared.locator.ordinal,
                    shared.locator.identity.clone(),
                ),
                total: row.total,
                cells: row.grid.map(|grid| {
                    grid.cells
                        .into_iter()
                        .map(|observed| observed.cell(self.cumulative))
                        .collect()
                }),
            });
        }
        let grid = match &spec.query.view {
            HeatmapView::RankingOnly => None,
            HeatmapView::Grid { group, .. } => {
                debug_assert!(group.is_empty(), "ungrouped result carried group fields");
                let other_cells: Vec<Option<f64>> = totals
                    .iter()
                    .zip(&winner_sums)
                    .map(|(all, winners)| all.minus(winners))
                    .collect();
                Some(HeatmapGrid {
                    label_names: spec.labels.clone(),
                    group_names: Vec::new(),
                    intervals: intervals(self.range, self.columns),
                    groups: Vec::new(),
                    totals: HeatmapBand {
                        total: if additive_summary {
                            totals_total
                        } else {
                            band_peak(&totals)
                        },
                        cells: totals.iter().map(CellSum::value).collect(),
                    },
                    others: HeatmapBand {
                        total: if additive_summary {
                            others_total
                        } else {
                            peak_values(&other_cells)
                        },
                        cells: other_cells,
                    },
                })
            }
        };
        Ok(HeatmapItemResult {
            ranking,
            coverage,
            class: spec.class,
            summary,
            unit: spec.unit,
            entities,
            totals_total,
            others_total,
            entity_count,
            out_of_order: self.out_of_order,
            grid,
        })
    }

    fn finish_grouped(
        mut self,
        spec: &ItemSpec,
        coverage: HeatmapCoverage,
        ranking: super::query::NormalizedRanking,
        dictionary: &RenderedIds,
        section: &IndexedSection<'_>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        let mut totals = std::mem::take(&mut self.totals);
        let ordered = self.ordered_grid_folds(section);
        let mut group_totals = vec![None; self.groups.len()];
        let mut group_cells = vec![vec![CellSum::default(); self.columns]; self.groups.len()];
        for state in ordered {
            if let Some(finished) = state.current.cell(self.cumulative) {
                totals[state.column].add(finished);
            }
            let Some(group) = state.group else {
                continue;
            };
            if let Some(total) = self.score(state.entity, &state.window) {
                group_totals[group] = Some(group_totals[group].unwrap_or(0.0) + total);
            }
            for (sum, observed) in group_cells[group].iter_mut().zip(&state.cells) {
                if let Some(value) = observed.cell(self.cumulative) {
                    sum.add(value);
                }
            }
        }
        let order = group_order(&group_totals);
        let top = spec.query.ranking.top.min(order.len());
        let winners: HashSet<usize> = order.iter().take(top).copied().collect();
        let mut others = vec![CellSum::default(); self.columns];
        for group in order.iter().skip(top) {
            for (sum, value) in others.iter_mut().zip(&group_cells[*group]) {
                if let Some(value) = value.value() {
                    sum.add(value);
                }
            }
        }
        let mut groups = Vec::with_capacity(top);
        for group in order.iter().take(top) {
            let state = &self.groups[*group];
            groups.push(HeatmapGroup {
                values: render_cells(
                    &state.values,
                    state.segment_slot,
                    dictionary,
                    self.first_index,
                )?,
                members: state.members,
                total: group_totals[*group],
                cells: group_cells[*group].iter().map(CellSum::value).collect(),
            });
        }
        let totals_total = if self.additive_summary() {
            summary_total(group_totals.iter().flatten().copied(), true)
        } else {
            band_peak(&totals)
        };
        let others_total = if self.additive_summary() {
            summary_total(
                order
                    .iter()
                    .filter(|group| !winners.contains(group))
                    .filter_map(|group| group_totals[*group]),
                true,
            )
        } else {
            band_peak(&others)
        };
        let grid = Some(HeatmapGrid {
            label_names: spec.labels.clone(),
            group_names: spec.query.view.groups().to_vec(),
            intervals: intervals(self.range, self.columns),
            groups,
            totals: HeatmapBand {
                total: totals_total,
                cells: totals.iter().map(CellSum::value).collect(),
            },
            others: HeatmapBand {
                total: others_total,
                cells: others.iter().map(CellSum::value).collect(),
            },
        });
        Ok(HeatmapItemResult {
            ranking,
            coverage,
            class: spec.class,
            summary: self.summary(),
            unit: spec.unit,
            entities: Vec::new(),
            totals_total,
            others_total,
            entity_count: u64::try_from(self.groups.len()).unwrap_or(u64::MAX),
            out_of_order: self.out_of_order,
            grid,
        })
    }
}

struct RankedState {
    key: String,
    entity: EntityId,
    total: Option<f64>,
    identity_values: Vec<Value>,
    grid: Option<GridFold>,
}

fn compare_totals(left: Option<&f64>, right: Option<&f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.partial_cmp(left).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn group_order(totals: &[Option<f64>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..totals.len()).collect();
    order.sort_by(|left, right| {
        compare_totals(totals[*left].as_ref(), totals[*right].as_ref())
            .then_with(|| left.cmp(right))
    });
    order
}

fn ranking_reaches_cutoff(total: Option<f64>, cutoff: LabelCutoff) -> bool {
    match cutoff {
        LabelCutoff::Value(cutoff) if !cutoff.is_finite() => true,
        LabelCutoff::Value(cutoff) => {
            total.is_some_and(|total| !total.is_finite() || total >= cutoff)
        }
        LabelCutoff::Null => true,
    }
}

fn summary_total(values: impl Iterator<Item = f64>, additive: bool) -> Option<f64> {
    values.fold(None, |current, value| {
        Some(match current {
            Some(current) if !additive => current.max(value),
            Some(current) => current + value,
            None => value,
        })
    })
}

fn identity_object(type_id: u32, values: Vec<Value>) -> NamedValues {
    let names = contract(type_id)
        .map(|contract| contract.identity)
        .unwrap_or_default();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let name = names.get(index).map_or_else(
                || format!("value_{index}"),
                |name| public_identity_name(name).to_owned(),
            );
            (name, value)
        })
        .collect()
}

fn public_identity_name(name: &str) -> &str {
    IDENTITY_ALIASES
        .iter()
        .find(|(recorded, _public)| recorded == &name)
        .map_or(name, |(_recorded, public)| public)
}

fn labels_object(
    spec: &ItemSpec,
    labels: &[Option<StoredLabel>],
    label_slots: &[usize],
    dictionary: &RenderedIds,
    index: usize,
) -> Result<NamedValues, HeatmapError> {
    spec.labels
        .iter()
        .zip(label_slots)
        .map(|(name, slot)| {
            let stored = &labels[*slot];
            let value = stored.as_ref().map_or(Ok(Value::Null), |stored| {
                render_cell(&stored.value, stored.segment_slot, dictionary, index)
            })?;
            Ok((name.clone(), value))
        })
        .collect()
}

fn render_cells(
    cells: &[Cell],
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Vec<Value>, HeatmapError> {
    cells
        .iter()
        .map(|stored| render_cell(stored, segment_slot, dictionary, index))
        .collect()
}

fn render_cell(
    stored: &Cell,
    segment_slot: usize,
    dictionary: &RenderedIds,
    index: usize,
) -> Result<Value, HeatmapError> {
    if let Cell::StrId(id) = stored {
        return dictionary
            .get(&(segment_slot, *id))
            .cloned()
            .ok_or_else(|| HeatmapError::failure(index, format!("unresolved dictionary id {id}")));
    }
    cell(stored, &Dictionary::default()).map_err(|error| HeatmapError::storage(index, error))
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

#[cfg(test)]
mod tests;

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

fn intervals(range: crate::TimeRange, columns: usize) -> Vec<HeatmapInterval> {
    (0..columns)
        .map(|index| HeatmapInterval {
            start: interval_start(range, columns, index),
            end: interval_start(range, columns, index + 1).saturating_sub(1),
        })
        .collect()
}

pub(super) fn interval_start(range: crate::TimeRange, columns: usize, index: usize) -> i64 {
    let span = i128::from(range.to_exclusive) - i128::from(range.from);
    let offset = span * to_i128(index) / to_i128(columns.max(1));
    range.from.saturating_add(clamped(offset))
}

pub(super) fn column_of_span(
    previous_ts: Option<i64>,
    timestamp: i64,
    range: crate::TimeRange,
    columns: usize,
) -> usize {
    let middle = match previous_ts {
        Some(previous) if previous < timestamp => previous + (timestamp - previous) / 2,
        _ => timestamp,
    };
    column_of(middle.max(range.from), range, columns)
}

pub(super) fn column_of(timestamp: i64, range: crate::TimeRange, columns: usize) -> usize {
    let span = (i128::from(range.to_exclusive) - i128::from(range.from)).max(1);
    let offset = i128::from(timestamp) - i128::from(range.from);
    let column = (offset * to_i128(columns.max(1)) / span).max(0);
    usize::try_from(column)
        .unwrap_or_else(|_| columns.saturating_sub(1))
        .min(columns.saturating_sub(1))
}

fn to_i128(value: usize) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

fn clamped(offset: i128) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
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

impl Obs {
    pub(super) fn observe(&mut self, timestamp: i64, value: f64) {
        if self.count == 0 {
            *self = Self {
                count: 1,
                first_ts: timestamp,
                first_value: value,
                last_ts: timestamp,
                last_value: value,
                max_value: value,
            };
            return;
        }
        self.count = self.count.saturating_add(1);
        if timestamp < self.first_ts {
            self.first_ts = timestamp;
            self.first_value = value;
        }
        if timestamp >= self.last_ts {
            self.last_ts = timestamp;
            self.last_value = value;
        }
        if value > self.max_value {
            self.max_value = value;
        }
    }

    pub(super) fn cell(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.last_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        if delta < 0.0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "an interval of 2^52 microseconds is 142 years"
        )]
        let seconds = (self.last_ts - self.first_ts) as f64 / 1_000_000.0;
        Some(delta / seconds)
    }

    pub(super) fn total(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.max_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        (delta >= 0.0).then_some(delta)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct CellSum {
    sum: f64,
    contributors: u32,
}

impl CellSum {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.contributors = self.contributors.saturating_add(1);
    }

    fn value(&self) -> Option<f64> {
        (self.contributors > 0).then_some(self.sum)
    }

    fn minus(&self, winners: &Self) -> Option<f64> {
        let contributors = self.contributors.saturating_sub(winners.contributors);
        (contributors > 0).then_some(self.sum - winners.sum)
    }
}

fn band_peak(cells: &[CellSum]) -> Option<f64> {
    cells
        .iter()
        .filter_map(CellSum::value)
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}

fn peak_values(cells: &[Option<f64>]) -> Option<f64> {
    cells
        .iter()
        .flatten()
        .copied()
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}

fn stream_grid(
    item: &HeatmapItemResult,
    from: i64,
    to: i64,
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let grid = item.grid.as_ref().ok_or_else(|| {
        QueryError::BadFilter("streamed heatmap requires a grid result".to_owned())
    })?;
    let grouped = !grid.group_names.is_empty();
    let mut header = json!({
        "record": "heatmap",
        "from": from.to_string(),
        "to": to.to_string(),
        "section": item.ranking.section,
        "fields": item.ranking.fields,
        "class": item.class.code(),
        "summary": item.summary,
        "labels": grid.label_names,
        "top": if grouped { grid.groups.len() } else { item.entities.len() },
        "entity_count": item.entity_count,
        "others_count": item.entity_count.saturating_sub(
            u64::try_from(if grouped { grid.groups.len() } else { item.entities.len() })
                .unwrap_or(u64::MAX)
        ),
        "out_of_order": item.out_of_order.to_string(),
        "intervals": grid.intervals,
    });
    if grouped {
        let Some(header) = header.as_object_mut() else {
            return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "heatmap header did not serialize as an object",
            ))));
        };
        header.insert("group".to_owned(), json!(grid.group_names));
    }
    if sink.cancelled() || !sink.record(record(header)?) {
        return Ok(());
    }
    if grouped {
        for group in &grid.groups {
            if sink.cancelled()
                || !sink.record(record(json!({
                    "record": "heatmap_row",
                    "type_id": "0",
                    "identity": group.values,
                    "labels": [],
                    "members": group.members,
                    "total": group.total,
                    "cells": group.cells,
                }))?)
            {
                return Ok(());
            }
        }
    } else {
        for entity in &item.entities {
            let type_id = entity.detail_locator.type_id;
            let labels: Vec<Value> = grid
                .label_names
                .iter()
                .map(|name| entity.labels.get(name).cloned().unwrap_or(Value::Null))
                .collect();
            if sink.cancelled()
                || !sink.record(record(json!({
                    "record": "heatmap_row",
                    "type_id": type_id.to_string(),
                    "identity": stream_identity(type_id, &entity.identity),
                    "labels": labels,
                    "total": entity.total,
                    "cells": entity.cells,
                }))?)
            {
                return Ok(());
            }
        }
    }
    if !sink.record(record(json!({
        "record": "heatmap_band",
        "band": "totals",
        "total": grid.totals.total,
        "cells": grid.totals.cells,
    }))?) {
        return Ok(());
    }
    if !sink.record(record(json!({
        "record": "heatmap_band",
        "band": "others",
        "total": grid.others.total,
        "cells": grid.others.cells,
    }))?) {
        return Ok(());
    }
    Ok(())
}

fn stream_identity(type_id: u32, identity: &BTreeMap<String, Value>) -> Vec<Value> {
    let Some(contract) = contract(type_id) else {
        return identity.values().cloned().collect();
    };
    contract
        .identity
        .iter()
        .map(|name| {
            identity
                .get(public_identity_name(name))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect()
}
