//! Tablespace histories assembled across database partitions.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::Segment;
use kronika_registry::{ColumnClass, contract, logical_section_name};
use serde_json::json;

use super::{process_tablespace_history_chunk, relation_layout, relation_record};

use crate::hour::relation::fields::physical_fields;
use crate::hour::relation::{
    HISTORY_CHUNK_ROWS, HistoryMoment, HistoryPrevious, HistorySegment, IdentityCell,
    RelationAggregate, RelationKind, RelationRow, RelationSource, rate_columns, timestamp_cell,
    unsigned_cell,
};
use crate::projection::{Plan, plans};
use crate::render::record;
use crate::{
    DataRequest, DatasetSegment, Filter, HourSeriesRequest, QueryDataset, QueryError, QuerySink,
    RelationGroup, SegmentRequest, Window,
};

#[allow(
    clippy::too_many_lines,
    reason = "the cross-database as-of reducer keeps scan and emission state adjacent"
)]
pub(super) fn stream_tablespace_history(
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
