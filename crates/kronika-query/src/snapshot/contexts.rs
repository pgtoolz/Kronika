//! Snapshot row windows and per-partition predecessor contexts.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use kronika_reader::Segment;
use kronika_registry::ColumnClass;

use super::{
    CachedFact, IdentityCell, Moments, PageContext, PageFacts, PageOrder, PartitionMoments,
    PartitionRateState, PartitionSource, PreparedSnapshot, Readings, RowWindow, SectionPlans,
    SelectedPartition, SnapshotViewSpec, cgroup, identity_cell, identity_of,
    projected_rate_columns, row_timestamp, rows_of,
};

use crate::QueryError;
use crate::dataset::DatasetSegment;
use crate::projection::Plan;
use crate::snapshot::cursor::{partition_context_index, timed_context_index};
use crate::snapshot::filter::{clock_ticks_per_second, postgres_block_size, search_columns};
use crate::snapshot::paging::page_order;
use crate::snapshot::predecessor::located_moment;

impl PreparedSnapshot {
    pub(super) fn page_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        if SnapshotViewSpec::for_logical_name(&section.logical_name).is_some() {
            return self.partitioned_contexts(section, cancelled);
        }
        let mut contexts = Vec::with_capacity(section.plans.len());
        let mut facts = HashMap::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if (!plan.applies() && !self.latest && cgroup::legacy(&section.logical_name).is_none())
                || cancelled()
            {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                let facts = self.page_facts(&section.logical_name, &self.anchor, &mut facts)?;
                contexts.push(PageContext {
                    context_index: layout_index,
                    plan,
                    logical_name: &section.logical_name,
                    source: &self.anchor,
                    rows: rows_of(&self.anchor, plan.type_id).unwrap_or(0),
                    window: RowWindow::Untimed,
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: None,
                    order: page_order(&section.logical_name, plan, &self.by),
                    search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                        search_columns(&section.logical_name, plan, search)
                    }),
                    clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                    block_size: facts.block_size.value(),
                });
                continue;
            };
            contexts.extend(self.timed_contexts(
                section,
                layout_index,
                plan,
                timestamp,
                cancelled,
                &mut facts,
            )?);
        }
        cgroup::retain_family(&mut contexts, &section.logical_name);
        if self.latest && !self.pin_current {
            // Physical revisions belong to one logical snapshot. Keep equal-time
            // contributors, but do not page or emit an older revision's sample.
            let latest = contexts
                .iter()
                .filter_map(|context| context.sample_to)
                .max();
            contexts.retain(|context| context.sample_to.is_none() || context.sample_to == latest);
        }
        Ok(contexts)
    }

    pub(super) fn page_facts(
        &self,
        logical_name: &str,
        source_ref: &DatasetSegment,
        cached: &mut HashMap<i64, PageFacts>,
    ) -> Result<PageFacts, QueryError> {
        if !matches!(
            logical_name,
            "os_process" | "pg_stat_statements" | "pg_store_plans"
        ) {
            return Ok(PageFacts::default());
        }
        let mut facts = cached.get(&source_ref.id()).copied().unwrap_or_default();
        let present = if logical_name == "os_process" {
            facts.clock_ticks_per_second != CachedFact::Unknown
        } else {
            facts.block_size != CachedFact::Unknown
        };
        if present {
            return Ok(facts);
        }
        let source = self.dataset.open(source_ref)?;
        if logical_name == "os_process" {
            facts.clock_ticks_per_second =
                clock_ticks_per_second(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        } else {
            facts.block_size =
                postgres_block_size(&source)?.map_or(CachedFact::Absent, CachedFact::Value);
        }
        cached.insert(source_ref.id(), facts);
        Ok(facts)
    }

    pub(super) fn first_match_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            let Some(timestamp) = plan.timestamp else {
                return Err(QueryError::BadFilter("first_match".to_owned()));
            };
            let mut moments = Vec::new();
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moment) = Self::moments(&source, plan, timestamp, self.at, cancelled)? {
                    moments.push((source_index, moment.current));
                }
            }
            let Some(current) = moments.iter().map(|(_source, at)| *at).max() else {
                continue;
            };
            for (source_index, at) in moments {
                if at != current {
                    continue;
                }
                let source = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                contexts.push(PageContext {
                    context_index: timed_context_index(
                        layout_index,
                        source_index,
                        self.source_count(),
                    ),
                    plan,
                    logical_name: &section.logical_name,
                    source,
                    rows: rows_of(source, plan.type_id).unwrap_or(0),
                    window: RowWindow::Shared { timestamp, current },
                    previous: None,
                    elapsed: None,
                    elapsed_by_partition: Arc::new(BTreeMap::new()),
                    sample_from: None,
                    sample_to: Some(current),
                    order: None,
                    search_columns: Vec::new(),
                    clock_ticks_per_second: None,
                    block_size: None,
                });
            }
        }
        Ok(contexts)
    }

    pub(super) fn timed_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        timestamp: &'static str,
        cancelled: &(impl Fn() -> bool + ?Sized),
        facts: &mut HashMap<i64, PageFacts>,
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let Some(moments) = self.shared_moments(plan, timestamp, cancelled)? else {
            return Ok(Vec::new());
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        if let Some(before) = moments.previous {
            for source_index in (0..self.source_count()).rev() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_some() {
                    let source = self.dataset.open(source_ref)?;
                    previous.extend(Self::collect(
                        &source,
                        plan,
                        timestamp,
                        before,
                        &order_columns,
                        cancelled,
                    )?);
                }
            }
        }
        let previous = Arc::new(previous);
        let elapsed = moments
            .previous
            .and_then(|before| moments.current.checked_sub(before))
            .filter(|elapsed| *elapsed > 0);
        let mut contexts = Vec::new();
        for source_index in 0..self.source_count() {
            let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
            if rows_of(source_ref, plan.type_id).is_none() {
                continue;
            }
            let source = self.dataset.open(source_ref)?;
            if Self::moments(&source, plan, timestamp, moments.current, cancelled)?
                .is_none_or(|source_moments| source_moments.current != moments.current)
            {
                continue;
            }
            let facts = self.page_facts(&section.logical_name, source_ref, facts)?;
            contexts.push(PageContext {
                context_index: timed_context_index(layout_index, source_index, self.source_count()),
                plan,
                logical_name: &section.logical_name,
                source: source_ref,
                rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
                window: RowWindow::Shared {
                    timestamp,
                    current: moments.current,
                },
                previous: Some(Arc::clone(&previous)),
                elapsed,
                elapsed_by_partition: Arc::new(BTreeMap::new()),
                sample_from: moments.previous,
                sample_to: Some(moments.current),
                order: order.clone(),
                search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                    search_columns(&section.logical_name, plan, search)
                }),
                clock_ticks_per_second: facts.clock_ticks_per_second.value(),
                block_size: facts.block_size.value(),
            });
        }
        Ok(contexts)
    }

    pub(super) fn shared_moments(
        &self,
        plan: &Plan,
        timestamp: &'static str,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Option<Moments>, QueryError> {
        let anchor = self.dataset.open(&self.anchor)?;
        let here = Self::moments(&anchor, plan, timestamp, self.at, cancelled)?;
        drop(anchor);
        let mut current = here.map(|moments| moments.current);
        if current.is_none() || !self.pin_current {
            for source_ref in &self.prior_sources {
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, self.at, cancelled)?
                {
                    current = Some(
                        current.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
        }
        let Some(current) = current else {
            return Ok(None);
        };
        if self.current_from.is_some_and(|from| current < from) {
            return Ok(None);
        }
        let mut previous = None;
        if let Some(before) = current.checked_sub(1) {
            for source_index in 0..self.source_count() {
                let source_ref = self.source_for(source_index).ok_or(QueryError::BadCursor)?;
                if rows_of(source_ref, plan.type_id).is_none() {
                    continue;
                }
                let source = self.dataset.open(source_ref)?;
                if let Some(moments) = Self::moments(&source, plan, timestamp, before, cancelled)? {
                    previous = Some(
                        previous.map_or(moments.current, |chosen: i64| chosen.max(moments.current)),
                    );
                }
            }
        }
        Ok(Some(Moments { current, previous }))
    }

    pub(super) fn partitioned_contexts<'a>(
        &'a self,
        section: &'a SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Vec<PageContext<'a>>, QueryError> {
        let Some(spec) = SnapshotViewSpec::for_logical_name(&section.logical_name) else {
            return Ok(Vec::new());
        };
        let selected = self.selected_partitions(section, cancelled);
        if cancelled() {
            return Ok(Vec::new());
        }
        let mut contexts = Vec::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || plan.timestamp.is_none() {
                continue;
            }
            let rate_state =
                self.partition_rate_state(section, layout_index, plan, &selected, spec, cancelled)?;
            for source in std::iter::once(PartitionSource::Current).chain(
                (0..self.relation_predecessors.len())
                    .rev()
                    .map(PartitionSource::Earlier),
            ) {
                if let Some(context) = self.partitioned_context(
                    section,
                    layout_index,
                    plan,
                    source,
                    &selected,
                    &rate_state,
                    spec,
                ) {
                    contexts.push(context);
                }
            }
        }
        Ok(contexts)
    }

    pub(super) fn selected_partitions(
        &self,
        section: &SectionPlans,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> BTreeMap<IdentityCell, SelectedPartition> {
        let source_by_id = std::iter::once((self.anchor.id(), PartitionSource::Current))
            .chain(
                self.relation_predecessors
                    .iter()
                    .enumerate()
                    .map(|(index, source)| (source.id(), PartitionSource::Earlier(index))),
            )
            .collect::<HashMap<_, _>>();
        let mut selected = BTreeMap::<IdentityCell, SelectedPartition>::new();
        for (layout_index, plan) in section.plans.iter().enumerate() {
            if !plan.applies() || cancelled() {
                continue;
            }
            if plan.timestamp.is_none() {
                continue;
            }
            for ((type_id, partition), retained) in &self.relation_moments {
                if *type_id != plan.type_id {
                    continue;
                }
                let Some(current) = retained
                    .current
                    .as_ref()
                    .and_then(|moment| located_moment(moment, &source_by_id))
                else {
                    continue;
                };
                if self.current_from.is_some_and(|from| current.at < from) {
                    continue;
                }
                let moments = PartitionMoments {
                    current,
                    previous: retained
                        .previous
                        .as_ref()
                        .and_then(|moment| located_moment(moment, &source_by_id)),
                };
                let candidate = SelectedPartition {
                    layout_index,
                    type_id: plan.type_id,
                    moments,
                };
                let replace = selected.get(partition).is_none_or(|chosen| {
                    (candidate.moments.current.at, candidate.type_id)
                        > (chosen.moments.current.at, chosen.type_id)
                });
                if replace {
                    selected.insert(partition.clone(), candidate);
                }
            }
        }
        selected
    }

    pub(super) fn partition_rate_state(
        &self,
        section: &SectionPlans,
        layout_index: usize,
        plan: &Plan,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        spec: SnapshotViewSpec,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<PartitionRateState, QueryError> {
        let Some(timestamp) = plan.timestamp else {
            return Err(QueryError::BadCursor);
        };
        let order = page_order(&section.logical_name, plan, &self.by);
        let order_columns = order.as_ref().map_or_else(Vec::new, PageOrder::columns);
        let mut previous = Readings::new();
        let mut elapsed_by_partition = BTreeMap::new();
        let mut sample_from: Option<i64> = None;
        let mut sample_to: Option<i64> = None;
        let mut before_by_source =
            BTreeMap::<i64, (&DatasetSegment, BTreeMap<IdentityCell, i64>)>::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index {
                continue;
            }
            sample_to = Some(sample_to.map_or(selection.moments.current.at, |chosen| {
                chosen.max(selection.moments.current.at)
            }));
            let Some(before) = selection.moments.previous.as_ref() else {
                continue;
            };
            let Some(elapsed) = selection
                .moments
                .current
                .at
                .checked_sub(before.at)
                .filter(|elapsed| *elapsed > 0)
            else {
                continue;
            };
            for source in &before.sources {
                if let Some(reference) = self.partition_source(*source) {
                    before_by_source
                        .entry(reference.id())
                        .or_insert_with(|| (reference, BTreeMap::new()))
                        .1
                        .insert(partition.clone(), before.at);
                }
            }
            elapsed_by_partition.insert(partition.clone(), elapsed);
            sample_from = Some(sample_from.map_or(before.at, |chosen| chosen.min(before.at)));
        }
        for (_segment_id, (before_source_ref, partitions)) in before_by_source {
            let before_source = self.dataset.open(before_source_ref)?;
            previous.extend(Self::collect_partitions(
                &before_source,
                plan,
                timestamp,
                spec.temporal_partition,
                &partitions,
                &order_columns,
                cancelled,
            )?);
        }
        Ok(PartitionRateState {
            previous: Arc::new(previous),
            elapsed_by_partition: Arc::new(elapsed_by_partition),
            sample_from,
            sample_to,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the selected layout, source, and partition contract define one context"
    )]
    pub(super) fn partitioned_context<'a>(
        &'a self,
        section: &'a SectionPlans,
        layout_index: usize,
        plan: &'a Plan,
        source_kind: PartitionSource,
        selected: &BTreeMap<IdentityCell, SelectedPartition>,
        rate_state: &PartitionRateState,
        spec: SnapshotViewSpec,
    ) -> Option<PageContext<'a>> {
        let timestamp = plan.timestamp?;
        if !selected.values().any(|selection| {
            selection.layout_index == layout_index
                && selection.moments.current.sources.contains(&source_kind)
        }) {
            return None;
        }
        let source_ref = self.partition_source(source_kind)?;
        let order = page_order(&section.logical_name, plan, &self.by);
        let mut current = BTreeMap::new();
        for (partition, selection) in selected {
            if selection.layout_index != layout_index
                || !selection.moments.current.sources.contains(&source_kind)
            {
                continue;
            }
            current.insert(partition.clone(), selection.moments.current.at);
        }
        Some(PageContext {
            context_index: partition_context_index(
                layout_index,
                source_kind,
                self.relation_predecessors.len(),
            ),
            plan,
            logical_name: &section.logical_name,
            source: source_ref,
            rows: rows_of(source_ref, plan.type_id).unwrap_or(0),
            window: RowWindow::Partitioned {
                timestamp,
                column: spec.temporal_partition,
                current,
            },
            previous: Some(Arc::clone(&rate_state.previous)),
            elapsed: None,
            elapsed_by_partition: Arc::clone(&rate_state.elapsed_by_partition),
            sample_from: rate_state.sample_from,
            sample_to: rate_state.sample_to,
            order,
            search_columns: self.search.as_ref().map_or_else(Vec::new, |search| {
                search_columns(&section.logical_name, plan, search)
            }),
            clock_ticks_per_second: None,
            block_size: None,
        })
    }

    pub(super) const fn source_count(&self) -> usize {
        self.prior_sources.len() + 1
    }

    pub(super) fn source_for(&self, source_index: usize) -> Option<&DatasetSegment> {
        if source_index == 0 {
            Some(&self.anchor)
        } else {
            self.prior_sources.get(source_index - 1)
        }
    }

    pub(super) fn partition_source(&self, source: PartitionSource) -> Option<&DatasetSegment> {
        match source {
            PartitionSource::Current => Some(&self.anchor),
            PartitionSource::Earlier(index) => self.relation_predecessors.get(index),
        }
    }

    pub(super) fn collect_partitions(
        segment: &Segment,
        plan: &Plan,
        timestamp: &'static str,
        partition_column: &'static str,
        partitions: &BTreeMap<IdentityCell, i64>,
        extra_columns: &[&'static str],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<Readings, QueryError> {
        #[cfg(test)]
        PARTITION_PREDECESSOR_VISITS.set(PARTITION_PREDECESSOR_VISITS.get().saturating_add(1));
        let mut collected = BTreeMap::new();
        let mut counters = projected_rate_columns(plan);
        if plan.contract.column("starttime").is_some() {
            counters.push("starttime");
        }
        for column in extra_columns {
            if plan
                .contract
                .column(column)
                .is_some_and(|declared| declared.class == ColumnClass::Cumulative)
                && !counters.contains(column)
            {
                counters.push(column);
            }
        }
        if counters.is_empty() {
            return Ok(collected);
        }
        let mut projection = counters.clone();
        projection.extend(plan.contract.identity.iter().copied());
        projection.extend([timestamp, partition_column]);
        projection.sort_unstable();
        projection.dedup();
        segment.visit_rows(plan.type_id, &projection, 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            let Some(partition) = row.get(partition_column).map(identity_cell) else {
                return true;
            };
            let Some(at) = partitions.get(&partition) else {
                return true;
            };
            if row_timestamp(&row, timestamp) != Some(*at) {
                return true;
            }
            let Some(key) = identity_of(plan, &row) else {
                return true;
            };
            let mut stored = BTreeMap::new();
            for name in &counters {
                if let Some(value) = row.get(name) {
                    stored.insert(*name, value.clone());
                }
            }
            collected.insert(key, stored);
            true
        })?;
        Ok(collected)
    }
}

#[cfg(test)]
use super::PARTITION_PREDECESSOR_VISITS;
