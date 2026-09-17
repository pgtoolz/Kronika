//! Snapshot validation, projection, and captured selection.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use kronika_reader::{Segment, SegmentKind};
use kronika_registry::{contract, logical_section_name, registry};

use super::{
    CPU_TIME_VIRTUAL_FIELD, PROCESS_USER_VIRTUAL_FIELDS, PROCESS_VIRTUAL_FIELDS, PageOrder,
    PreparedSnapshot, SectionPlans, SnapshotCursor, SnapshotViewSpec, cgroup, relation,
    row_timestamp,
};

use crate::dataset::{DatasetSegment, QueryDataset, SegmentBounds, SegmentSelection};
use crate::snapshot::cursor::{pin, snapshot_binding};
use crate::snapshot::filter::{search_clause_columns, search_columns};
use crate::snapshot::paging::page_order;
use crate::snapshot::predecessor::{preceding, relation_preceding};
use crate::snapshot::search::StructuredSearch;
use crate::{
    DataRequest, Prepared, QueryContext, QueryError, QueryExecution, QueryIdentity, QueryMetadata,
    QueryStability, RelationGroup, SegmentRequest, SnapshotRequest, StatementScope,
    output_fields as shared_fields,
};

/// Snapshot selection and validator facts available before predecessor scans.
#[derive(Debug)]
pub struct SnapshotPreparation {
    dataset: Arc<dyn QueryDataset>,
    anchor: DatasetSegment,
    segments: Vec<DatasetSegment>,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
    cursor: Option<SnapshotCursor>,
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
    binding: u64,
    stability: QueryStability,
    validator_shape: String,
    validator_segments: Vec<DatasetSegment>,
}

struct PreparedSnapshotInputs {
    cursor: Option<SnapshotCursor>,
    search: Option<Box<StructuredSearch>>,
    first_match_query_id: Option<i64>,
    binding: u64,
}

/// Capture one explicit snapshot selection without scanning predecessors.
///
/// # Errors
///
/// Returns a query error when the request is invalid, its segment is absent,
/// or the captured dataset cannot provide a consistent catalog.
pub fn prepare_snapshot(
    context: &QueryContext,
    request: SnapshotRequest,
) -> Result<SnapshotPreparation, QueryError> {
    let inputs = prepared_snapshot_inputs(&request)?;
    let listing = {
        let catalog = context.dataset.catalog()?;
        catalog.segments(SegmentSelection::new(SegmentBounds::all()))?
    };
    let clean = listing.warnings.is_empty();
    let mut segments = listing.segments;
    let index = segments
        .iter()
        .position(|segment| segment.id() == request.segment_id)
        .ok_or(QueryError::NoSuchSegment)?;
    let current = segments.remove(index);
    prepare_selected_state_with_inputs(
        Arc::clone(&context.dataset),
        current,
        segments,
        clean,
        request,
        true,
        None,
        inputs,
    )
}

pub(crate) fn prepare_selected(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    current_from: Option<i64>,
) -> Result<PreparedSnapshot, QueryError> {
    prepare_selected_state(
        dataset,
        current,
        segments,
        clean,
        request,
        false,
        current_from,
    )?
    .finish_prepared()
}

pub(super) fn prepare_selected_state(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
) -> Result<SnapshotPreparation, QueryError> {
    let inputs = prepared_snapshot_inputs(&request)?;
    prepare_selected_state_with_inputs(
        dataset,
        current,
        segments,
        clean,
        request,
        pin_current,
        current_from,
        inputs,
    )
}

fn prepared_snapshot_inputs(
    request: &SnapshotRequest,
) -> Result<PreparedSnapshotInputs, QueryError> {
    let cursor = request
        .cursor
        .as_deref()
        .map(SnapshotCursor::parse)
        .transpose()?;
    let search = prepared_search(request, cursor.is_some())?;
    let first_match_query_id = prepared_first_match(request, search.as_deref())?;
    let binding = snapshot_binding(request, search.as_deref());
    let parsed = cursor
        .filter(|cursor| cursor.segment_id == request.segment_id && cursor.binding == binding);
    if request.cursor.is_some() && parsed.is_none() {
        return Err(QueryError::BadCursor);
    }
    Ok(PreparedSnapshotInputs {
        cursor: parsed,
        search,
        first_match_query_id,
        binding,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "captured selection and parsed request state meet at this ownership boundary"
)]
fn prepare_selected_state_with_inputs(
    dataset: Arc<dyn QueryDataset>,
    current: DatasetSegment,
    segments: Vec<DatasetSegment>,
    clean: bool,
    request: SnapshotRequest,
    pin_current: bool,
    current_from: Option<i64>,
    inputs: PreparedSnapshotInputs,
) -> Result<SnapshotPreparation, QueryError> {
    let PreparedSnapshotInputs {
        cursor,
        search,
        first_match_query_id,
        binding,
    } = inputs;
    let anchor = pin(dataset.as_ref(), current, cursor)?;
    let active_position = anchor.active_position().unwrap_or(0);
    if cursor.is_some_and(|cursor| cursor.active_position != active_position) {
        return Err(QueryError::BadCursor);
    }
    let validator_segments =
        std::iter::once(&anchor)
            .chain(segments.iter().filter(|candidate| {
                candidate.id() < anchor.id() && candidate.min_ts() <= request.at
            }))
            .cloned()
            .collect::<Vec<_>>();
    let immutable = clean
        && anchor.kind() == SegmentKind::Finished
        && validator_segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
    let stability = match anchor.kind() {
        SegmentKind::Active => QueryStability::Mutable,
        SegmentKind::Finished if immutable => QueryStability::Immutable,
        SegmentKind::Finished => QueryStability::Revalidate,
    };
    let validator_shape = format!("{request:?}");
    Ok(SnapshotPreparation {
        dataset,
        anchor,
        segments,
        request,
        pin_current,
        current_from,
        cursor,
        search,
        first_match_query_id,
        binding,
        stability,
        validator_shape,
        validator_segments,
    })
}

impl SnapshotPreparation {
    /// Stability and immutable identity for HTTP cache adaptation.
    #[must_use]
    pub fn metadata(&self) -> QueryMetadata<'_> {
        QueryMetadata {
            stability: self.stability,
            identity: (self.stability == QueryStability::Immutable).then_some(
                QueryIdentity::SegmentSet {
                    resource: "snapshot",
                    shape: &self.validator_shape,
                    segments: &self.validator_segments,
                },
            ),
        }
    }

    /// Finish planning and predecessor scans, producing the shared execution.
    ///
    /// # Errors
    ///
    /// Returns a query error when projection planning, segment opening, or
    /// predecessor selection fails.
    pub fn finish(self) -> Result<QueryExecution, QueryError> {
        Ok(QueryExecution {
            prepared: Prepared::Snapshot(self.finish_prepared()?),
        })
    }

    pub(super) fn finish_prepared(self) -> Result<PreparedSnapshot, QueryError> {
        let Self {
            dataset,
            anchor,
            segments,
            request,
            pin_current,
            current_from,
            cursor,
            search,
            first_match_query_id,
            binding,
            stability,
            validator_shape,
            validator_segments,
        } = self;
        let segment = dataset.open(&anchor)?;
        let relation_fields = request
            .group
            .map(|group| shared_fields(&request.sections, group, &request.fields))
            .transpose()?
            .unwrap_or_default();
        let (physical_filters, relation_filters) = relation::split_filters(&request)?;
        let mut physical_request = request.clone();
        physical_request.filters = physical_filters;
        let mut sections = section_plans(
            &segment,
            &physical_request,
            &relation_fields,
            search.as_deref(),
        )?;
        cgroup::extend_plans(
            dataset.as_ref(),
            &anchor,
            &segments,
            &physical_request,
            &mut sections,
            search.as_deref(),
        )?;
        project_statement_text(&mut sections, request.scope);
        let relation_count = sections
            .iter()
            .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some())
            .count();
        let prior_sources = if relation_count < sections.len() {
            preceding(
                dataset.as_ref(),
                &anchor,
                segments.clone(),
                &segment,
                &sections,
                request.at,
                pin_current,
            )?
        } else {
            Vec::new()
        };
        let (relation_predecessors, relation_moments) = if relation_count > 0 {
            relation_preceding(
                dataset.as_ref(),
                &anchor,
                segments,
                &segment,
                &sections,
                &physical_request.filters,
                request.at,
            )?
        } else {
            (Vec::new(), BTreeMap::new())
        };
        validate_search_projection(search.as_deref(), &sections)?;
        validate_exact_locator(&segment, &request, &sections)?;
        drop(segment);
        Ok(PreparedSnapshot {
            dataset,
            anchor,
            pin_current,
            prior_sources,
            relation_predecessors,
            relation_moments,
            at: request.at,
            current_from,
            sections,
            relation_filters,
            by: request.by,
            direction: request.direction,
            group: request.group,
            relation_fields,
            page_size: request.page_size,
            cursor,
            binding,
            search,
            first_match_query_id,
            text: request.text,
            row_ordinal: request.row_ordinal,
            scope: request.scope,
            stability,
            validator_shape,
            validator_segments,
        })
    }
}

fn prepared_first_match(
    request: &SnapshotRequest,
    search: Option<&StructuredSearch>,
) -> Result<Option<i64>, QueryError> {
    if !request.first_match {
        return Ok(None);
    }
    search
        .and_then(StructuredSearch::first_match_query_id)
        .map(Some)
        .ok_or_else(|| QueryError::BadFilter("first_match".to_owned()))
}

pub(super) fn prepared_search(
    request: &SnapshotRequest,
    cursor_present: bool,
) -> Result<Option<Box<StructuredSearch>>, QueryError> {
    let parsed = request
        .search
        .as_deref()
        .map(|raw| {
            let [logical_name] = request.sections.as_slice() else {
                return Err(QueryError::BadFilter("search".to_owned()));
            };
            let search = StructuredSearch::parse(raw, logical_name).map_err(|_diagnostic| {
                if cursor_present {
                    QueryError::BadCursor
                } else {
                    QueryError::BadFilter("search".to_owned())
                }
            })?;
            if request
                .group
                .is_some_and(|group| group != RelationGroup::Object)
            {
                search.validate_grouped_phase().map_err(|_diagnostic| {
                    if cursor_present {
                        QueryError::BadCursor
                    } else {
                        QueryError::BadFilter("search".to_owned())
                    }
                })?;
            }
            Ok(search)
        })
        .transpose()?;
    Ok(parsed.map(Box::new))
}

/// A scoped statement page reads the statement text so the collector's own
/// rows can be recognised while the page is ranked.
fn project_statement_text(sections: &mut [SectionPlans], scope: StatementScope) {
    for section in sections {
        if !scope.filters(&section.logical_name) {
            continue;
        }
        for plan in &mut section.plans {
            if let Some(column) = plan.contract.column("query") {
                plan.add_projection_columns(&[column.name]);
            }
        }
    }
}

fn selected_virtual_fields<'a>(logical_name: &str, fields: &'a [String]) -> Vec<&'a str> {
    let known: &[&str] = match logical_name {
        "os_process" => PROCESS_VIRTUAL_FIELDS,
        "os_cgroup_v2_cpu" => &[
            "throttled_period_ratio",
            "quota_cores",
            "throttled_interval",
        ],
        "os_cgroup_v2_memory" => &["local_oom_kill_delta"],
        "os_cgroup_v2_pids" => &["failure_max_delta"],
        _ => &[],
    };
    if fields.is_empty() {
        known.to_vec()
    } else {
        fields
            .iter()
            .filter_map(|field| known.contains(&field.as_str()).then_some(field.as_str()))
            .collect()
    }
}

pub(super) fn section_plans(
    segment: &Segment,
    request: &SnapshotRequest,
    relation_fields: &[String],
    search: Option<&StructuredSearch>,
) -> Result<Vec<SectionPlans>, QueryError> {
    let shared_projection = request.sections.len() > 1 && !request.fields.is_empty();
    if shared_projection {
        validate_shared_projection(&request.sections, &request.fields)?;
    }
    let mut sections = Vec::with_capacity(request.sections.len());
    for logical_name in &request.sections {
        let fields = if let Some(group) = request.group {
            let wanted = relation::snapshot_physical_fields(
                logical_name,
                group,
                relation_fields,
                &request.by,
                search,
            )?;
            section_projection(segment, logical_name, &wanted)
        } else if shared_projection {
            section_projection(segment, logical_name, &request.fields)
        } else {
            request.fields.clone()
        };
        if shared_projection && fields.is_empty() {
            continue;
        }
        let selected_virtual = selected_virtual_fields(logical_name, &fields);
        let physical_fields = fields
            .iter()
            .filter(|field| {
                !PROCESS_VIRTUAL_FIELDS.contains(&field.as_str())
                    && !selected_virtual.contains(&field.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        let data = DataRequest {
            segment: SegmentRequest {
                segment_id: request.segment_id,
                section: logical_name.clone(),
            },
            fields: physical_fields.clone(),
            filters: request.filters.clone(),
            type_id: request.type_id,
            after: None,
        };
        // Missing sections are empty so one source cannot fail the snapshot.
        match cgroup::plans(segment, &data) {
            Ok(mut plans) => {
                for plan in &mut plans {
                    if !fields.is_empty() {
                        plan.retain_output_fields(&physical_fields);
                    }
                    for field in &selected_virtual {
                        plan.add_virtual_output(field);
                    }
                    if !fields.is_empty() {
                        plan.order_output_fields(&fields);
                    }
                    if selected_virtual
                        .iter()
                        .any(|field| PROCESS_USER_VIRTUAL_FIELDS.contains(field))
                    {
                        plan.add_projection_columns(&["uid", "euid", "scope"]);
                    }
                    if selected_virtual.contains(&CPU_TIME_VIRTUAL_FIELD) {
                        plan.add_projection_columns(&["utime", "stime"]);
                    }
                    cgroup::project_virtual_inputs(logical_name, plan);
                    if logical_name == "pg_store_plans" {
                        plan.add_aliased_output("calls_per_second", "calls");
                    }
                    let order = page_order(logical_name, plan, &request.by);
                    plan.add_projection_columns(
                        &order.as_ref().map_or_else(Vec::new, PageOrder::columns),
                    );
                    if logical_name == "os_process" && plan.contract.column("starttime").is_some() {
                        plan.add_projection_columns(&["starttime"]);
                    }
                    if let Some(search) = search {
                        plan.add_projection_columns(&search_columns(logical_name, plan, search));
                    }
                }
                sections.push(SectionPlans {
                    logical_name: logical_name.clone(),
                    plans,
                });
            }
            Err(QueryError::NoSuchSection) if cgroup::legacy(logical_name).is_some() => {
                sections.push(SectionPlans {
                    logical_name: logical_name.clone(),
                    plans: Vec::new(),
                });
            }
            Err(QueryError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(sections)
}
pub(super) fn validate_search_projection(
    search: Option<&StructuredSearch>,
    sections: &[SectionPlans],
) -> Result<(), QueryError> {
    if let Some(search) = search {
        let [section] = sections else {
            return Err(QueryError::BadFilter("search".to_owned()));
        };
        if !section.plans.iter().any(|plan| {
            search.member_clauses().all(|clause| {
                !search_clause_columns(&section.logical_name, plan, clause.key).is_empty()
            })
        }) {
            return Err(QueryError::BadFilter("search".to_owned()));
        }
    }
    Ok(())
}

fn validate_exact_locator(
    segment: &Segment,
    request: &SnapshotRequest,
    sections: &[SectionPlans],
) -> Result<(), QueryError> {
    if let Some(ordinal) = request.row_ordinal {
        let [section] = sections else {
            return Err(QueryError::BadCursor);
        };
        let [plan] = section.plans.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let Some(timestamp) = plan.timestamp else {
            return Err(QueryError::BadCursor);
        };
        if ordinal >= plan.rows {
            return Err(QueryError::BadCursor);
        }
        let mut exact = false;
        segment.visit_rows(plan.type_id, &[timestamp], ordinal, 1, |_stored, row| {
            exact = row_timestamp(&row, timestamp) == Some(request.at);
            false
        })?;
        if !exact {
            return Err(QueryError::BadCursor);
        }
    }
    Ok(())
}
/// A shared field must exist in some registered layout of a requested section,
/// whatever the segment recorded, as in `projection::plans`.
fn validate_shared_projection(sections: &[String], fields: &[String]) -> Result<(), QueryError> {
    for field in fields {
        let known = registry().iter().any(|layout| {
            logical_section_name(layout.type_id.get()).is_some_and(|name| {
                sections
                    .iter()
                    .any(|section| section == name || cgroup::legacy(section) == Some(name))
            }) && layout.column(field).is_some()
        });
        let cgroup_virtual = sections.iter().any(|section| {
            cgroup::legacy(section).is_some()
                && !selected_virtual_fields(section, std::slice::from_ref(field)).is_empty()
        });
        if !known && !cgroup_virtual {
            return Err(QueryError::NoSuchColumn(field.clone()));
        }
    }
    Ok(())
}

fn section_projection(segment: &Segment, logical_name: &str, fields: &[String]) -> Vec<String> {
    let columns = cgroup::legacy(logical_name).map_or_else(
        || {
            segment
                .layouts(logical_name)
                .filter_map(|(type_id, _section)| contract(type_id))
                .flat_map(|layout| layout.columns.iter().map(|column| column.name))
                .collect::<HashSet<_>>()
        },
        |legacy| {
            registry()
                .iter()
                .filter(|layout| {
                    let name = logical_section_name(layout.type_id.get());
                    name == Some(logical_name) || name == Some(legacy)
                })
                .flat_map(|layout| layout.columns.iter().map(|column| column.name))
                .collect::<HashSet<_>>()
        },
    );
    let virtual_fields = if cgroup::legacy(logical_name).is_some() {
        selected_virtual_fields(logical_name, fields)
    } else {
        Vec::new()
    };
    fields
        .iter()
        .filter(|field| {
            columns.contains(field.as_str()) || virtual_fields.contains(&field.as_str())
        })
        .cloned()
        .collect()
}
