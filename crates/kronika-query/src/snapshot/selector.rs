//! Sample selection shared by the nine current-state finders.

use crate::StatementScope;
use std::sync::Arc;

use kronika_reader::Cell;
use kronika_registry::{contract, logical_section_name, registry};

use super::relation::RelationRow;
use super::{PlainRowOut, PreparedSnapshot, ProcessRowOut, StructuredSearch};
use crate::{
    DatasetSegment, Order, PredecessorSelection, QueryContext, QueryDataset, QueryError,
    RelationGroup, RelationKind, SegmentBounds, SegmentSelection, SnapshotRequest,
};

const SECOND_MICROS: i64 = 1_000_000;
const MIN_LOOKBACK_MICROS: i64 = 20 * SECOND_MICROS;
const DEFAULT_POSTGRESQL_CADENCE_SECONDS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SamplePolicy {
    logical_name: &'static str,
    fixed_cadence_seconds: Option<u64>,
}

const DERIVED_SORT_TOKENS: [(&str, &str); 7] = [
    (
        "derived_mean_exec_ms_per_call",
        "derived.mean_exec_ms_per_call",
    ),
    ("derived_rows_per_call", "derived.rows_per_call"),
    ("derived_blocks_per_call", "derived.blocks_per_call"),
    ("derived_hit_fraction", "derived.hit_pct"),
    ("derived_wal_per_call", "derived.wal_per_call"),
    ("derived_plan_time_fraction", "derived.plan_time_pct"),
    ("derived_cv", "derived.cv"),
];

/// Timestamp selection for one current-state finder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPoint {
    /// Select the latest recorded timestamp across the captured catalog.
    LatestRecorded,
    /// Select the latest eligible sample at or before this timestamp.
    At(i64),
}

/// One of the nine typed current-state finder surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinderSurface {
    /// Operating-system processes.
    Processes,
    /// `PostgreSQL` user tables.
    Tables,
    /// `PostgreSQL` user indexes.
    Indexes,
    /// `PostgreSQL` activity.
    Activity,
    /// `PostgreSQL` locks.
    Locks,
    /// `PostgreSQL` vacuum progress.
    Vacuum,
    /// `PostgreSQL` database statistics.
    Databases,
    /// `PostgreSQL` statement statistics.
    Statements,
    /// `PostgreSQL` stored plans.
    Plans,
}

impl FinderSurface {
    /// Registry logical-section name selected by this finder.
    #[must_use]
    pub const fn logical_name(self) -> &'static str {
        self.policy().logical_name
    }

    const fn policy(self) -> SamplePolicy {
        match self {
            Self::Processes => SamplePolicy {
                logical_name: "os_process",
                fixed_cadence_seconds: Some(5),
            },
            Self::Tables => SamplePolicy {
                logical_name: "pg_stat_user_tables",
                fixed_cadence_seconds: Some(300),
            },
            Self::Indexes => SamplePolicy {
                logical_name: "pg_stat_user_indexes",
                fixed_cadence_seconds: Some(300),
            },
            Self::Activity => SamplePolicy {
                logical_name: "pg_stat_activity",
                fixed_cadence_seconds: None,
            },
            Self::Locks => SamplePolicy {
                logical_name: "pg_locks",
                fixed_cadence_seconds: None,
            },
            Self::Vacuum => SamplePolicy {
                logical_name: "pg_stat_progress_vacuum",
                fixed_cadence_seconds: None,
            },
            Self::Databases => SamplePolicy {
                logical_name: "pg_stat_database",
                fixed_cadence_seconds: None,
            },
            Self::Statements => SamplePolicy {
                logical_name: "pg_stat_statements",
                fixed_cadence_seconds: None,
            },
            Self::Plans => SamplePolicy {
                logical_name: "pg_store_plans",
                fixed_cadence_seconds: None,
            },
        }
    }

    const fn relation_kind(self) -> Option<RelationKind> {
        match self {
            Self::Tables => Some(RelationKind::Tables),
            Self::Indexes => Some(RelationKind::Indexes),
            _ => None,
        }
    }

    fn order_token(self, group: Option<RelationGroup>, field: &str) -> Result<String, QueryError> {
        if let Some(kind) = self.relation_kind() {
            let group = group.ok_or_else(|| QueryError::BadFilter("group".to_owned()))?;
            return kind
                .sort_field_known(group, field)
                .then(|| field.to_owned())
                .ok_or_else(|| QueryError::NoSuchColumn(field.to_owned()));
        }
        if matches!(self, Self::Statements | Self::Plans)
            && let Some((_, token)) = DERIVED_SORT_TOKENS
                .iter()
                .find(|(public, _token)| *public == field)
        {
            return Ok((*token).to_owned());
        }
        registry()
            .iter()
            .filter(|layout| {
                logical_section_name(layout.type_id.get()) == Some(self.logical_name())
            })
            .any(|layout| layout.column(field).is_some())
            .then(|| field.to_owned())
            .ok_or_else(|| QueryError::NoSuchColumn(field.to_owned()))
    }
}

/// Optional semantic ordering for one current-state finder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderOrder {
    /// Public finder field name.
    pub field: String,
    /// Shared direction for the selected field.
    pub direction: Order,
}

/// Typed request accepted by the nine current-state finders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderQuery {
    /// Finder family to execute.
    pub surface: FinderSurface,
    /// Recorded timestamp selection.
    pub point: SnapshotPoint,
    /// Optional parsed, bounded search.
    pub search: Option<StructuredSearch>,
    /// Optional semantic order.
    pub order: Option<FinderOrder>,
    /// Required aggregation level for relation finders.
    pub group: Option<RelationGroup>,
    /// Maximum returned rows.
    pub limit: usize,
}

/// Exact newest-section request used by adapters outside the nine finder surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentSnapshotQuery {
    /// Registry logical-section name.
    pub logical_name: String,
    /// Public output fields selected for relation aggregation.
    pub fields: Vec<String>,
    /// Optional semantic result ordering.
    pub order: Option<FinderOrder>,
    /// Optional relation aggregation level.
    pub group: Option<RelationGroup>,
    /// Maximum returned rows.
    pub limit: usize,
}

/// Typed rows and pagination facts returned by a current-state finder.
#[derive(Debug)]
pub struct FinderResult<R> {
    /// Selected rows in exact result order.
    pub rows: Vec<R>,
    /// Whether another matching row existed beyond the requested limit.
    pub truncated: bool,
    /// Latest selected sample timestamp, when any row was eligible.
    pub as_of: Option<i64>,
}

impl<R> FinderResult<R> {
    const fn empty() -> Self {
        Self {
            rows: Vec::new(),
            truncated: false,
            as_of: None,
        }
    }
}

/// Execute the process finder through the shared snapshot engine.
///
/// # Errors
///
/// Returns a typed request, captured-data, or cancellation error.
pub fn execute_processes(
    context: &QueryContext,
    query: &FinderQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<FinderResult<ProcessRowOut>, QueryError> {
    if query.surface != FinderSurface::Processes {
        return Err(QueryError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    let Some(prepared) = prepare(context, query, cancelled)? else {
        return Ok(FinderResult::empty());
    };
    prepared.compute_process_rows(limit, cancelled)
}

/// Execute one non-relation `PostgreSQL` finder through the shared snapshot engine.
///
/// # Errors
///
/// Returns a typed request, captured-data, or cancellation error.
pub fn execute_plain(
    context: &QueryContext,
    query: &FinderQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<FinderResult<PlainRowOut>, QueryError> {
    if matches!(
        query.surface,
        FinderSurface::Processes | FinderSurface::Tables | FinderSurface::Indexes
    ) {
        return Err(QueryError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    let Some(prepared) = prepare(context, query, cancelled)? else {
        return Ok(FinderResult::empty());
    };
    prepared.compute_plain_rows(limit, cancelled)
}

/// Execute one grouped relation finder through the shared snapshot engine.
///
/// # Errors
///
/// Returns a typed request, captured-data, or cancellation error.
pub fn execute_relation(
    context: &QueryContext,
    query: &FinderQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<FinderResult<RelationRow>, QueryError> {
    if !matches!(
        query.surface,
        FinderSurface::Tables | FinderSurface::Indexes
    ) {
        return Err(QueryError::BadFilter("surface".to_owned()));
    }
    let limit = query.limit;
    let Some(prepared) = prepare(context, query, cancelled)? else {
        return Ok(FinderResult::empty());
    };
    prepared.compute_relation_rows(limit, cancelled)
}

/// Execute the newest segment carrying one plain section.
///
/// # Errors
///
/// Returns a typed request, captured-data, or cancellation error.
pub fn execute_current_plain(
    context: &QueryContext,
    query: CurrentSnapshotQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<FinderResult<PlainRowOut>>, QueryError> {
    let limit = query.limit;
    let Some(prepared) = prepare_current(context, query, cancelled)? else {
        return Ok(None);
    };
    prepared.compute_plain_rows(limit, cancelled).map(Some)
}

/// Execute the newest segment carrying one grouped relation section.
///
/// # Errors
///
/// Returns a typed request, captured-data, or cancellation error.
pub fn execute_current_relation(
    context: &QueryContext,
    query: CurrentSnapshotQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<FinderResult<RelationRow>>, QueryError> {
    let limit = query.limit;
    let Some(prepared) = prepare_current(context, query, cancelled)? else {
        return Ok(None);
    };
    prepared.compute_relation_rows(limit, cancelled).map(Some)
}

fn prepare_current(
    context: &QueryContext,
    query: CurrentSnapshotQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<PreparedSnapshot>, QueryError> {
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    let dataset = Arc::clone(&context.dataset);
    let catalog = dataset.catalog()?;
    let listing = catalog.segments(SegmentSelection::new(SegmentBounds::all()))?;
    let clean = listing.warnings.is_empty();
    let mut segments = listing.segments;
    let Some(index) = selected_segment(&segments, &query.logical_name) else {
        return Ok(None);
    };
    let anchor = segments.remove(index);
    let at = anchor.max_ts();
    let direction = query
        .order
        .as_ref()
        .map_or(Order::Asc, |order| order.direction);
    let by = query.order.into_iter().map(|order| order.field).collect();
    let request = SnapshotRequest {
        segment_id: anchor.id(),
        at,
        sections: vec![query.logical_name],
        fields: query.fields,
        by,
        direction,
        group: query.group,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
        scope: StatementScope::All,
    };
    drop(catalog);
    super::prepare_selected_state(dataset, anchor, segments, clean, request, true, None)?
        .finish_prepared()
        .map(Some)
}

fn prepare(
    context: &QueryContext,
    query: &FinderQuery,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<PreparedSnapshot>, QueryError> {
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    let by = query
        .order
        .as_ref()
        .map(|order| query.surface.order_token(query.group, &order.field))
        .transpose()?
        .into_iter()
        .collect();
    let direction = query
        .order
        .as_ref()
        .map_or(Order::Asc, |order| order.direction);
    let dataset = Arc::clone(&context.dataset);
    let catalog = dataset.catalog()?;
    let at = match query.point {
        SnapshotPoint::LatestRecorded => {
            let Some(at) = catalog.ranges().iter().map(|(_from, to)| *to).max() else {
                return Ok(None);
            };
            at
        }
        SnapshotPoint::At(at) => at,
    };
    let policy = query.surface.policy();
    let cadence = if let Some(cadence) = policy.fixed_cadence_seconds {
        cadence
    } else {
        let probe = catalog.segments(SegmentSelection {
            bounds: SegmentBounds::inclusive(Some(at), Some(at)),
            predecessor: PredecessorSelection::ForLayouts(physical_type_ids("instance_metadata")),
        })?;
        recorded_postgresql_cadence(dataset.as_ref(), &probe.segments, at, cancelled)?
            .unwrap_or(DEFAULT_POSTGRESQL_CADENCE_SECONDS)
    };
    let lookback = cadence_lookback(cadence)?;
    let current_from = at.checked_sub(lookback).unwrap_or(i64::MIN);
    let listing = catalog.segments(SegmentSelection {
        bounds: SegmentBounds::inclusive(Some(current_from), Some(at)),
        predecessor: PredecessorSelection::ForLayouts(physical_type_ids(policy.logical_name)),
    })?;
    let clean = listing.warnings.is_empty();
    let mut segments = listing.segments;
    let Some(index) = selected_segment(&segments, query.surface.logical_name()) else {
        return Ok(None);
    };
    let anchor = segments.remove(index);
    let request = SnapshotRequest {
        segment_id: anchor.id(),
        at,
        sections: vec![query.surface.logical_name().to_owned()],
        fields: Vec::new(),
        by,
        direction,
        group: query.group,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
        scope: StatementScope::All,
    };
    drop(catalog);
    let prepared = super::prepare_selected(
        dataset,
        anchor,
        segments,
        clean,
        request,
        Some(current_from),
    )?;
    prepared.with_search(query.search.clone()).map(Some)
}

fn selected_segment(segments: &[DatasetSegment], logical_name: &str) -> Option<usize> {
    segments
        .iter()
        .enumerate()
        .filter(|(_index, segment)| {
            segment
                .sections()
                .iter()
                .any(|section| logical_section_name(section.type_id) == Some(logical_name))
        })
        .max_by_key(|(_index, segment)| segment.id())
        .map(|(index, _segment)| index)
}

fn physical_type_ids(logical_name: &str) -> Vec<u32> {
    registry()
        .iter()
        .filter(|layout| logical_section_name(layout.type_id.get()) == Some(logical_name))
        .map(|layout| layout.type_id.get())
        .collect()
}

fn cadence_lookback(cadence_seconds: u64) -> Result<i64, QueryError> {
    let cadence = i64::try_from(cadence_seconds)
        .ok()
        .and_then(|seconds| seconds.checked_mul(2_500_000))
        .ok_or_else(|| {
            QueryError::Unreadable(Box::new(std::io::Error::other(
                "recorded PostgreSQL interval is too large",
            )))
        })?;
    Ok(cadence.max(MIN_LOOKBACK_MICROS))
}

fn recorded_postgresql_cadence(
    dataset: &dyn QueryDataset,
    segments: &[DatasetSegment],
    at: i64,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<u64>, QueryError> {
    let mut selected: Option<(i64, u64)> = None;
    for segment_ref in segments {
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        let segment = dataset.open(segment_ref)?;
        for (type_id, _rows) in segment.sections() {
            if logical_section_name(type_id) != Some("instance_metadata") {
                continue;
            }
            let Some(layout) = contract(type_id) else {
                continue;
            };
            let (Some(timestamp), Some(interval)) = (
                layout.column("ts"),
                layout.column("postgresql_interval_seconds"),
            ) else {
                continue;
            };
            segment.visit_rows(
                type_id,
                &[timestamp.name, interval.name],
                0,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        return false;
                    }
                    let (Some(Cell::Ts(timestamp)), Some(Cell::U64(seconds))) =
                        (row.get(timestamp.name), row.get(interval.name))
                    else {
                        return true;
                    };
                    if *timestamp <= at && *seconds > 0 {
                        let candidate = (*timestamp, *seconds);
                        if selected.is_none_or(|current| candidate.0 > current.0) {
                            selected = Some(candidate);
                        }
                    }
                    true
                },
            )?;
            if cancelled() {
                return Err(QueryError::Cancelled);
            }
        }
    }
    Ok(selected.map(|(_timestamp, seconds)| seconds))
}

#[cfg(test)]
mod tests;
