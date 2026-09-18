//! Relation history selection, scanning, and record emission.

mod tablespace;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Row, Segment};
use kronika_registry::{ColumnClass, contract, logical_section_name};
use serde_json::{Map, Value, json};
use tablespace::stream_tablespace_history;

use super::{
    GroupKey, GroupKeyValue, HISTORY_CHUNK_ROWS, HistoryMoment, HistoryPrevious, HistorySegment,
    IdentityCell, Metric, RelationAggregate, RelationKind, RelationRow, RelationSource,
    identity_of, rate_columns, timestamp_cell, unsigned_cell,
};

use crate::hour::relation::fields::{kind_name, output_fields, physical_fields};
use crate::projection::{Plan, chunk_dictionary, plans};
use crate::render::record;
use crate::{
    DataRequest, DatasetSegment, Filter, HourSeriesRequest, QueryDataset, QueryError, QuerySink,
    RelationGroup, SegmentRequest, Window,
};

#[expect(
    clippy::too_many_lines,
    reason = "one grouped stream keeps validation, the fixed two passes, and ordered emission together"
)]
pub(in crate::hour) fn stream_history(
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

pub(super) fn relation_values(metrics: &BTreeMap<String, Option<Metric>>) -> Map<String, Value> {
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
