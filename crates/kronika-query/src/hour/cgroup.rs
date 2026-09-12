use std::collections::{BTreeMap, BTreeSet};

use kronika_reader::{Cell, Segment};
use kronika_registry::{contract, logical_section_name};

use crate::projection::plans;
use crate::{
    DataRequest, DatasetSegment, HourSeriesRequest, QueryDataset, QueryError, QuerySink,
    SegmentRequest, Window,
};

type Moments = BTreeMap<i64, BTreeSet<u32>>;

pub(super) fn breaks(
    dataset: &dyn QueryDataset,
    segments: &[DatasetSegment],
    window: Window,
    series: &HourSeriesRequest,
    sink: &dyn QuerySink,
) -> Result<BTreeSet<(u32, i64)>, QueryError> {
    let mut result = BTreeSet::new();
    let Some(type_id) = series.type_id else {
        return Ok(result);
    };
    let Some(layout) = contract(type_id) else {
        return Ok(result);
    };
    let resource = match series.section.as_str() {
        "os_cgroup_cpu" | "os_cgroup_v2_cpu" => "cpu",
        "os_cgroup_memory" | "os_cgroup_v2_memory" => "memory",
        "os_cgroup_pids" | "os_cgroup_v2_pids" => "pids",
        "os_cgroup_io" | "os_cgroup_v2_io" => "io",
        _ => return Ok(result),
    };
    if layout
        .identity
        .iter()
        .any(|column| !series.filters.iter().any(|filter| filter.column == *column))
    {
        return Ok(result);
    }
    let old = format!("os_cgroup_{resource}");
    let new = format!("os_cgroup_v2_{resource}");
    let wanted_source = series
        .filters
        .iter()
        .find(|filter| filter.column == "events_source")
        .and_then(|filter| filter.value.parse::<u32>().ok())
        .unwrap_or(0);
    let mut observed = Moments::new();
    let mut selected = Moments::new();
    for descriptor in segments {
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        let segment = dataset.open(descriptor)?;
        for (recorded, _) in segment.sections() {
            let name = logical_section_name(recorded);
            if name != Some(old.as_str()) && name != Some(new.as_str()) {
                continue;
            }
            if recorded == type_id {
                selected_moments(&segment, window, series, &mut observed, &mut selected, sink)?;
            } else {
                segment.visit_rows(recorded, &["ts"], 0, usize::MAX, |_, row| {
                    if let Some(Cell::Ts(at)) = row.get("ts")
                        && window.contains(*at)
                    {
                        observed.entry(*at).or_default().insert(recorded);
                    }
                    !sink.cancelled()
                })?;
            }
        }
    }
    let wanted_new = logical_section_name(type_id) == Some(new.as_str());
    let mut previous_eligible = false;
    let mut seen = false;
    for (at, types) in observed {
        let new_present = types
            .iter()
            .any(|type_id| logical_section_name(*type_id) == Some(new.as_str()));
        let has_row = selected
            .get(&at)
            .is_some_and(|sources| sources.contains(&wanted_source));
        let eligible = has_row && types.contains(&type_id) && new_present == wanted_new;
        if has_row && (!eligible || (seen && !previous_eligible)) {
            result.insert((type_id, at));
        }
        seen |= has_row;
        previous_eligible = eligible;
    }
    Ok(result)
}

fn selected_moments(
    segment: &Segment,
    window: Window,
    series: &HourSeriesRequest,
    observed: &mut Moments,
    selected: &mut Moments,
    sink: &dyn QuerySink,
) -> Result<(), QueryError> {
    let request = DataRequest {
        segment: SegmentRequest {
            segment_id: segment.id(),
            section: series.section.clone(),
        },
        fields: vec!["ts".to_owned()],
        filters: series
            .filters
            .iter()
            .filter(|filter| filter.column != "events_source")
            .cloned()
            .collect(),
        type_id: series.type_id,
        after: None,
    };
    for mut plan in plans(segment, &request, true)? {
        if plan.contract.column("events_source").is_some() {
            plan.add_projection_columns(&["events_source"]);
        }
        let dictionary = plan.exact_filter_dictionary(segment)?;
        segment.visit_rows(plan.type_id, &plan.projection, 0, usize::MAX, |_, row| {
            if let Some(Cell::Ts(at)) = row.get("ts")
                && window.contains(*at)
            {
                observed.entry(*at).or_default().insert(plan.type_id);
                if plan.matches(&row, &dictionary) {
                    let source = match row.get("events_source") {
                        Some(Cell::U32(source)) => *source,
                        _ => 0,
                    };
                    selected.entry(*at).or_default().insert(source);
                }
            }
            !sink.cancelled()
        })?;
    }
    Ok(())
}
