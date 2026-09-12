use super::{
    CounterReadings, PageContext, PageOrder, PageOrderKind, SectionPlans, StructuredSearch,
};
use crate::projection::Plan;
use crate::{DataRequest, DatasetSegment, QueryDataset, QueryError, SnapshotRequest};
use kronika_reader::{Cell, Row, Segment};
use kronika_registry::{logical_section_name, registry};
use serde_json::{Value, json};

pub(super) fn legacy(name: &str) -> Option<&'static str> {
    match name {
        "os_cgroup_v2_cpu" => Some("os_cgroup_cpu"),
        "os_cgroup_v2_memory" => Some("os_cgroup_memory"),
        "os_cgroup_v2_io" => Some("os_cgroup_io"),
        "os_cgroup_v2_pids" => Some("os_cgroup_pids"),
        _ => None,
    }
}

pub(super) fn plans(segment: &Segment, request: &DataRequest) -> Result<Vec<Plan>, QueryError> {
    let Some(old) = legacy(&request.segment.section) else {
        return crate::projection::plans(segment, request, true);
    };
    let known = registry()
        .iter()
        .filter(|layout| {
            let name = logical_section_name(layout.type_id.get());
            name == Some(request.segment.section.as_str()) || name == Some(old)
        })
        .collect::<Vec<_>>();
    for field in &request.fields {
        if known.iter().all(|layout| layout.column(field).is_none()) {
            return Err(QueryError::NoSuchColumn(field.clone()));
        }
    }
    crate::projection::validate_filters(&known, &request.filters)?;
    let mut result = Vec::new();
    for name in [request.segment.section.as_str(), old] {
        let mut physical = request.clone();
        name.clone_into(&mut physical.segment.section);
        let known = registry()
            .iter()
            .filter(|layout| logical_section_name(layout.type_id.get()) == Some(name))
            .collect::<Vec<_>>();
        let missing_filter = request.filters.iter().any(|filter| {
            known
                .iter()
                .all(|layout| layout.column(&filter.column).is_none())
        });
        physical.filters.retain(|filter| {
            known
                .iter()
                .any(|layout| layout.column(&filter.column).is_some())
        });
        physical
            .fields
            .retain(|field| known.iter().any(|layout| layout.column(field).is_some()));
        match crate::projection::plans(segment, &physical, true) {
            Ok(mut selected) => {
                if missing_filter {
                    for plan in &mut selected {
                        plan.exclude_rows();
                    }
                }
                if !request.fields.is_empty() {
                    for plan in &mut selected {
                        plan.retain_output_fields(&request.fields);
                        for field in &request.fields {
                            if plan.contract.column(field).is_none() {
                                plan.add_virtual_output(field);
                            }
                        }
                        plan.order_output_fields(&request.fields);
                    }
                }
                result.extend(selected);
            }
            Err(QueryError::NoSuchSection) => {}
            Err(error) => return Err(error),
        }
    }
    if result.is_empty() {
        Err(QueryError::NoSuchSection)
    } else {
        Ok(result)
    }
}

pub(super) fn extend_plans(
    dataset: &dyn QueryDataset,
    anchor: &DatasetSegment,
    candidates: &[DatasetSegment],
    request: &SnapshotRequest,
    sections: &mut Vec<SectionPlans>,
    search: Option<&StructuredSearch>,
) -> Result<(), QueryError> {
    let names = request
        .sections
        .iter()
        .filter(|name| legacy(name).is_some())
        .cloned()
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Ok(());
    }
    let mut selected = request.clone();
    selected.sections = names;
    for candidate in candidates {
        if candidate.id() >= anchor.id() || candidate.min_ts() > request.at {
            continue;
        }
        let missing = candidate.sections().iter().any(|layout| {
            selected.sections.iter().any(|name| {
                let physical = logical_section_name(layout.type_id);
                (physical == Some(name.as_str()) || physical == legacy(name))
                    && sections
                        .iter()
                        .flat_map(|section| &section.plans)
                        .all(|plan| plan.type_id != layout.type_id)
            })
        });
        if !missing {
            continue;
        }
        let source = dataset.open(candidate)?;
        for found in super::section_plans(&source, &selected, &[], search)? {
            if let Some(existing) = sections
                .iter_mut()
                .find(|section| section.logical_name == found.logical_name)
            {
                for plan in found.plans {
                    if existing
                        .plans
                        .iter()
                        .all(|present| present.type_id != plan.type_id)
                    {
                        existing.plans.push(plan);
                    }
                }
            } else {
                sections.push(found);
            }
        }
    }
    for section in sections {
        if legacy(&section.logical_name).is_some() {
            section.plans.sort_unstable_by_key(|plan| plan.type_id);
        }
    }
    Ok(())
}

pub(super) fn retain_family(contexts: &mut Vec<PageContext<'_>>, name: &str) {
    if legacy(name).is_none() {
        return;
    }
    let current = contexts
        .iter()
        .filter_map(|context| {
            context
                .sample_to
                .map(|at| (at, logical_section_name(context.plan.type_id) == Some(name)))
        })
        .max();
    let moments = contexts
        .iter()
        .filter_map(|context| context.sample_to.map(|at| (context.plan.type_id, at)))
        .collect::<std::collections::BTreeMap<_, _>>();
    contexts.retain(|context| {
        context
            .sample_to
            .map(|at| (at, logical_section_name(context.plan.type_id) == Some(name)))
            == current
    });
    for context in contexts {
        if context
            .sample_from
            .zip(context.sample_to)
            .is_some_and(|(from, to)| {
                moments
                    .iter()
                    .any(|(type_id, at)| *type_id != context.plan.type_id && *at > from && *at < to)
            })
        {
            context.previous = None;
            context.elapsed = None;
            context.sample_from = None;
        }
    }
}

pub(super) fn page_order(name: &str, plan: &Plan, token: &str) -> Option<PageOrder> {
    let interval = match (name, token) {
        ("os_cgroup_v2_cpu", "derived.throttled_interval") => Some("throttled_interval"),
        ("os_cgroup_v2_memory", "derived.local_oom_kill_delta") => Some("local_oom_kill_delta"),
        ("os_cgroup_v2_pids", "derived.failure_max_delta") => Some("failure_max_delta"),
        _ => None,
    };
    if let Some(name) = interval {
        let column = interval_column(name)?;
        return plan.contract.column(column).map(|_| PageOrder {
            name,
            kind: PageOrderKind::CounterDelta(column),
        });
    }
    if name != "os_cgroup_v2_cpu" {
        return None;
    }
    let (name, numerator, denominator, counter) = match token {
        "derived.throttled_period_ratio" => {
            ("throttled_period_ratio", "nr_throttled", "nr_periods", true)
        }
        "derived.quota_cores" => ("quota_cores", "quota_usec", "period_usec", false),
        _ => return None,
    };
    if plan.contract.column(numerator).is_none() || plan.contract.column(denominator).is_none() {
        return None;
    }
    Some(PageOrder {
        name,
        kind: if counter {
            PageOrderKind::CounterRatio {
                numerator: vec![numerator],
                denominator: vec![denominator],
                neutral_nulls: false,
            }
        } else {
            PageOrderKind::ValueRatio {
                numerator: vec![numerator],
                denominator: vec![denominator],
            }
        },
    })
}

pub(super) fn project_virtual_inputs(name: &str, plan: &mut Plan) {
    let columns: &[&str] = match name {
        "os_cgroup_v2_cpu" => &[
            "nr_periods",
            "nr_throttled",
            "quota_usec",
            "period_usec",
            "throttled_usec",
        ],
        "os_cgroup_v2_memory" => &["local_oom_kill"],
        "os_cgroup_v2_pids" => &["failure_max"],
        _ => &[],
    };
    let columns = columns
        .iter()
        .filter_map(|name| plan.contract.column(name).map(|column| column.name))
        .collect::<Vec<_>>();
    plan.add_projection_columns(&columns);
}

pub(super) fn virtual_type(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "quota_cores" | "throttled_period_ratio" => Some(("f64", "none")),
        "throttled_interval" => Some(("i64", "microseconds")),
        "local_oom_kill_delta" | "failure_max_delta" => Some(("i64", "count")),
        _ => None,
    }
}

pub(super) fn virtual_value(
    name: &str,
    row: &Row,
    before: Option<&CounterReadings>,
) -> Option<Value> {
    let interval_column = interval_column(name);
    if let Some(column) = interval_column {
        return Some(
            before
                .and_then(|before| super::counter_delta(row.get(column)?, before.get(column)?))
                .map_or(Value::Null, |delta| match delta {
                    super::OrderedNumber::Integer(delta) => json!(delta.to_string()),
                    super::OrderedNumber::Float(delta) => json!(delta),
                }),
        );
    }
    let value = match name {
        "quota_cores" => {
            let value = |column| match row.get(column) {
                Some(Cell::I64(value)) if *value > 0 => Some(*value),
                _ => None,
            };
            value("quota_usec")
                .zip(value("period_usec"))
                .map(|(quota, period)| {
                    super::OrderedNumber::Integer(i128::from(quota)).as_f64()
                        / super::OrderedNumber::Integer(i128::from(period)).as_f64()
                })
        }
        "throttled_period_ratio" => {
            let delta = |column| {
                super::counter_delta(row.get(column)?, before?.get(column)?)
                    .map(super::OrderedNumber::as_f64)
            };
            delta("nr_throttled")
                .zip(delta("nr_periods"))
                .and_then(|(throttled, periods)| (periods > 0.0).then_some(throttled / periods))
        }
        _ => return None,
    };
    Some(
        value
            .filter(|value| value.is_finite())
            .map_or(Value::Null, |value| json!(value)),
    )
}

fn interval_column(name: &str) -> Option<&'static str> {
    match name {
        "throttled_interval" => Some("throttled_usec"),
        "local_oom_kill_delta" => Some("local_oom_kill"),
        "failure_max_delta" => Some("failure_max"),
        _ => None,
    }
}
