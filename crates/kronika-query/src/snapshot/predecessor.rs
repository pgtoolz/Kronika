//! Predecessor selection across contributing snapshot partitions.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kronika_reader::Segment;

use super::{
    ContributingMoment, ContributingMoments, IdentityCell, LocatedMoment, PartitionSource,
    RetainedMoment, RetainedMoments, RetainedRelationMoments, SectionPlans, SnapshotViewSpec,
    cgroup, identity_cell, row_timestamp,
};

use crate::dataset::{DatasetSegment, QueryDataset};
use crate::projection::Plan;
use crate::{QueryError, SnapshotRequest};

pub(super) fn located_moment(
    retained: &RetainedMoment,
    source_by_id: &HashMap<i64, PartitionSource>,
) -> Option<LocatedMoment> {
    let sources = retained
        .segment_ids
        .iter()
        .filter_map(|segment_id| source_by_id.get(segment_id).copied())
        .collect::<BTreeSet<_>>();
    (!sources.is_empty()).then_some(LocatedMoment {
        at: retained.at,
        sources,
    })
}

pub(super) fn relation_preceding(
    dataset: &dyn QueryDataset,
    segment_ref: &DatasetSegment,
    segments: Vec<DatasetSegment>,
    current: &Segment,
    sections: &[SectionPlans],
    filters: &[crate::Filter],
    at: i64,
) -> Result<(Vec<DatasetSegment>, RetainedRelationMoments), QueryError> {
    let requested_datid = filters
        .iter()
        .find(|filter| filter.column == "datid")
        .and_then(|filter| filter.value.parse::<u32>().ok())
        .map(IdentityCell::U32);
    let mut moments = BTreeMap::<(u32, IdentityCell), RetainedMoments>::new();
    if let Some(datid) = requested_datid.as_ref() {
        for plan in partitioned_plans(sections) {
            if plan.applies() && plan.timestamp.is_some() {
                moments.entry((plan.type_id, datid.clone())).or_default();
            }
        }
    }
    scan_relation_moments(
        current,
        sections,
        requested_datid.as_ref(),
        requested_datid.is_none(),
        at,
        &mut moments,
    )?;
    let compatible = partitioned_plans(sections)
        .map(|plan| plan.type_id)
        .collect::<HashSet<_>>();
    let mut candidates = segments
        .into_iter()
        .filter(|candidate| candidate.id() < segment_ref.id() && candidate.min_ts() <= at)
        .filter(|candidate| {
            candidate
                .sections()
                .iter()
                .any(|section| compatible.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if !moments.is_empty()
            && moments.values().all(|samples| {
                samples
                    .previous
                    .as_ref()
                    .is_some_and(|previous| candidate.max_ts() < previous.at)
            })
        {
            break;
        }
        let segment = dataset.open(candidate)?;
        scan_relation_moments(
            &segment,
            sections,
            requested_datid.as_ref(),
            requested_datid.is_none(),
            at,
            &mut moments,
        )?;
    }

    let mut retained = HashSet::new();
    for samples in moments.values() {
        for sample in [&samples.current, &samples.previous].into_iter().flatten() {
            for segment_id in &sample.segment_ids {
                if *segment_id != segment_ref.id() {
                    retained.insert(*segment_id);
                }
            }
        }
    }
    let mut selected = candidates
        .into_iter()
        .filter(|candidate| retained.contains(&candidate.id()))
        .collect::<Vec<_>>();
    selected.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok((selected, moments))
}

fn partitioned_plans(sections: &[SectionPlans]) -> impl Iterator<Item = &Plan> {
    sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_some())
        .flat_map(|section| section.plans.iter())
}

fn scan_relation_moments(
    segment: &Segment,
    sections: &[SectionPlans],
    requested_datid: Option<&IdentityCell>,
    discover: bool,
    at: i64,
    moments: &mut BTreeMap<(u32, IdentityCell), RetainedMoments>,
) -> Result<(), QueryError> {
    for plan in partitioned_plans(sections) {
        if !plan.applies() {
            continue;
        }
        let Some(timestamp) = plan.timestamp else {
            continue;
        };
        if segment.rows_of(plan.type_id).is_none() {
            continue;
        }
        #[cfg(test)]
        RELATION_MOMENT_VISITS.set(RELATION_MOMENT_VISITS.get().saturating_add(1));
        segment.visit_rows(
            plan.type_id,
            &[timestamp, "datid"],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(stored), Some(partition)) =
                    (row_timestamp(&row, timestamp), row.get("datid"))
                else {
                    return true;
                };
                let partition = identity_cell(partition);
                if requested_datid.is_some_and(|requested| requested != &partition) {
                    return true;
                }
                let key = (plan.type_id, partition);
                if discover {
                    moments.entry(key.clone()).or_default();
                }
                if stored <= at
                    && let Some(selected) = moments.get_mut(&key)
                {
                    record_retained_moment(
                        selected,
                        RetainedMoment {
                            at: stored,
                            segment_ids: BTreeSet::from([segment.id()]),
                        },
                    );
                }
                true
            },
        )?;
    }
    Ok(())
}

fn record_retained_moment(moments: &mut RetainedMoments, sample: RetainedMoment) {
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(sample);
        return;
    };
    match sample.at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.extend(sample.segment_ids);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.replace(sample);
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match sample.at.cmp(&previous.at) {
                Ordering::Equal => previous.segment_ids.extend(sample.segment_ids),
                Ordering::Greater => *previous = sample,
                Ordering::Less => {}
            },
            None => moments.previous = Some(sample),
        },
    }
}

pub(super) fn preceding(
    dataset: &dyn QueryDataset,
    segment_ref: &DatasetSegment,
    segments: Vec<DatasetSegment>,
    current: &Segment,
    sections: &[SectionPlans],
    request: &SnapshotRequest,
    pin_current: bool,
) -> Result<Vec<DatasetSegment>, QueryError> {
    let layouts = sections
        .iter()
        .filter(|section| SnapshotViewSpec::for_logical_name(&section.logical_name).is_none())
        .flat_map(|section| {
            section.plans.iter().filter(move |plan| {
                plan.applies() || request.latest || cgroup::legacy(&section.logical_name).is_some()
            })
        })
        .filter_map(|plan| plan.timestamp.map(|timestamp| (plan.type_id, timestamp)))
        .collect::<BTreeMap<_, _>>();
    if layouts.is_empty() {
        return Ok(Vec::new());
    }
    let compatible = layouts.keys().copied().collect::<HashSet<_>>();
    let mut candidates = segments
        .into_iter()
        .filter(|candidate| candidate.id() < segment_ref.id() && candidate.min_ts() <= request.at)
        .filter(|candidate| {
            candidate
                .sections()
                .iter()
                .any(|section| compatible.contains(&section.type_id))
        })
        .collect::<Vec<_>>();
    let mut moments = layouts
        .keys()
        .map(|type_id| (*type_id, ContributingMoments::default()))
        .collect::<BTreeMap<_, _>>();
    scan_contributing_moments(current, &layouts, request.at, pin_current, &mut moments)?;
    candidates.sort_unstable_by_key(|candidate| Reverse((candidate.max_ts(), candidate.id())));
    for candidate in &candidates {
        if moments.values().all(|samples| {
            samples
                .previous
                .as_ref()
                .is_some_and(|previous| candidate.max_ts() < previous.at)
        }) {
            break;
        }
        let segment = dataset.open(candidate)?;
        scan_contributing_moments(&segment, &layouts, request.at, false, &mut moments)?;
    }
    let retained = moments
        .values()
        .flat_map(|moments| [&moments.current, &moments.previous])
        .filter_map(Option::as_ref)
        .flat_map(|moment| moment.segment_ids.iter().copied())
        .filter(|segment_id| *segment_id != segment_ref.id())
        .collect::<HashSet<_>>();
    candidates.retain(|candidate| retained.contains(&candidate.id()));
    candidates.sort_unstable_by_key(|candidate| Reverse(candidate.id()));
    Ok(candidates)
}

fn scan_contributing_moments(
    segment: &Segment,
    layouts: &BTreeMap<u32, &'static str>,
    at: i64,
    pin_current: bool,
    moments: &mut BTreeMap<u32, ContributingMoments>,
) -> Result<(), QueryError> {
    for (&type_id, &timestamp) in layouts {
        // Another layout may still need older sources. This one cannot gain a
        // newer sample, but equal-time sources must still contribute their rows.
        if moments
            .get(&type_id)
            .and_then(|samples| samples.previous.as_ref())
            .is_some_and(|previous| segment.max_ts() < previous.at)
        {
            continue;
        }
        if segment.rows_of(type_id).is_none() {
            continue;
        }
        segment.visit_rows(type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
            #[cfg(test)]
            super::CONTRIBUTING_MOMENT_ROWS.set(super::CONTRIBUTING_MOMENT_ROWS.get() + 1);
            if let Some(stored) = row_timestamp(&row, timestamp)
                && stored <= at
            {
                record_contributing_moment(
                    moments.entry(type_id).or_default(),
                    stored,
                    segment.id(),
                );
            }
            true
        })?;
    }
    if pin_current {
        for samples in moments.values_mut() {
            samples.pinned = samples.current.is_some();
        }
    }
    Ok(())
}

pub(super) fn record_contributing_moment(
    moments: &mut ContributingMoments,
    at: i64,
    segment_id: i64,
) {
    let new_moment = || ContributingMoment {
        at,
        segment_ids: HashSet::from([segment_id]),
    };
    let Some(current_at) = moments.current.as_ref().map(|current| current.at) else {
        moments.current = Some(new_moment());
        return;
    };
    if moments.pinned && at > current_at {
        return;
    }
    match at.cmp(&current_at) {
        Ordering::Equal => {
            if let Some(current) = moments.current.as_mut() {
                current.segment_ids.insert(segment_id);
            }
        }
        Ordering::Greater => {
            moments.previous = moments.current.take();
            moments.current = Some(new_moment());
        }
        Ordering::Less => match moments.previous.as_mut() {
            Some(previous) => match at.cmp(&previous.at) {
                Ordering::Equal => {
                    previous.segment_ids.insert(segment_id);
                }
                Ordering::Greater => moments.previous = Some(new_moment()),
                Ordering::Less => {}
            },
            None => moments.previous = Some(new_moment()),
        },
    }
}

#[cfg(test)]
use super::RELATION_MOMENT_VISITS;
