//! Heatmap entity accumulation, ranking, and grouped folds.

use std::collections::{HashMap, HashSet};

use kronika_reader::{Cell, Row};
use kronika_registry::ColumnClass;

use super::{
    Accumulator, Binding, CellSum, Edge, EntityId, FoldArena, GridCells, GridFold, GroupState,
    HeatmapError, IndexedSection, ItemSpec, LabelCutoff, Obs, RankFold, RankedState, RenderedIds,
    RssMean, ScanStats, SharedSectionSpec, entity_key_into, raw_key_into, reserve_id, reserve_ids,
    summed,
};

use crate::heatmap::execution::buckets::{
    band_peak, column_of, column_of_span, intervals, peak_values,
};
use crate::heatmap::execution::render::{
    compare_totals, group_order, identity_object, labels_object, ranking_reaches_cutoff,
    render_cells, summary_total,
};
use crate::heatmap::query::HeatmapView;
use crate::heatmap::result::{
    CoverageState, HeatmapBand, HeatmapCoverage, HeatmapEntity, HeatmapGrid, HeatmapGroup,
    HeatmapItemResult, HeatmapSummary,
};
use crate::row_key;

impl RssMean {
    pub(super) fn observe(&mut self, entity: EntityId, timestamp: i64, value: f64) {
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
    pub(super) fn mean(&self, entity: EntityId) -> Option<f64> {
        if self.timestamps.is_empty() {
            return None;
        }
        self.sums
            .get(entity.index())
            .map(|sum| sum / self.timestamps.len() as f64)
    }
}

const NO_FOLD: u32 = u32::MAX;

impl Accumulator {
    pub(super) fn new(
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
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    pub(super) fn observe(
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
        self.advance(fold, timestamp, value)
    }

    /// Fold an in-range sample into the grid and ranking window.
    fn advance(&mut self, fold: usize, timestamp: i64, value: f64) -> Result<(), HeatmapError> {
        let FoldArena::Grid { folds, .. } = &mut self.folds else {
            return Err(HeatmapError::invalid(
                self.first_index,
                "grid observation reached a non-grid accumulator",
            ));
        };
        let state = &mut folds[fold];
        if self.cumulative {
            if state.window.count > 0 {
                if timestamp < state.window.first_ts {
                    // An overlap may reveal an earlier endpoint late. Extend
                    // the known interval without counting its interior twice.
                    state.cells.observe_span(
                        (timestamp, value),
                        (state.window.first_ts, state.window.first_value),
                        self.range,
                    );
                } else if timestamp > state.window.last_ts {
                    state.cells.observe_span(
                        (state.window.last_ts, state.window.last_value),
                        (timestamp, value),
                        self.range,
                    );
                }
                if timestamp < state.window.last_ts {
                    self.out_of_order = self.out_of_order.saturating_add(1);
                }
            }
            state.window.observe(timestamp, value);
            return Ok(());
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
            if let GridCells::Gauges(cells) = &mut state.cells {
                cells[grid_column].observe(timestamp, value);
            }
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
            state.column = column;
            state.current = Obs::default();
        }
        state.current.observe(timestamp, value);
        Ok(())
    }

    pub(super) fn observe_edge(
        &mut self,
        row: &Row,
        timestamp: i64,
        entity: EntityId,
        binding: &Binding,
        edge: Edge,
    ) {
        if !(self.grid && self.cumulative) {
            return;
        }
        let Some(value) = summed(row, &binding.metrics) else {
            return;
        };
        let index = entity.index();
        let stored = match edge {
            Edge::Before => edge_slot(&mut self.before, index),
            Edge::After => edge_slot(&mut self.after, index),
        };
        let replace = match (edge, *stored) {
            (_, None) => true,
            (Edge::Before, Some((stored_ts, _value))) => stored_ts <= timestamp,
            (Edge::After, Some((stored_ts, _value))) => timestamp < stored_ts,
        };
        if replace {
            *stored = Some((timestamp, value));
        }
    }

    /// Add the two edge spans only after every candidate has been seen. An
    /// overlapping segment may provide the nearest sample late in the scan.
    fn close_edges(&mut self) {
        if !self.cumulative {
            return;
        }
        let FoldArena::Grid { folds, .. } = &mut self.folds else {
            return;
        };
        for fold in folds {
            if let Some(before) = self.before.get(fold.entity.index()).copied().flatten() {
                fold.cells.observe_span(
                    before,
                    (fold.window.first_ts, fold.window.first_value),
                    self.range,
                );
            }
            if let Some(after) = self.after.get(fold.entity.index()).copied().flatten() {
                fold.cells.observe_span(
                    (fold.window.last_ts, fold.window.last_value),
                    after,
                    self.range,
                );
            }
            // Derive all bands from the same completed cells as entity rows.
            for (sum, value) in self.totals.iter_mut().zip(fold.cells.values()) {
                if let Some(value) = value {
                    sum.add(value);
                }
            }
        }
    }

    pub(super) fn rank_fold(&mut self, entity: EntityId) -> Result<&mut RankFold, HeatmapError> {
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

    pub(super) fn grid_fold(
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
            cells: GridCells::new(self.columns, self.cumulative),
            grid_carry: None,
            group: None,
        });
        slot_by_entity[entity_index] = slot;
        let slot = usize::try_from(slot).map_err(|_error| {
            HeatmapError::invalid(self.first_index, "metric fold slot does not fit usize")
        })?;
        Ok((slot, true))
    }

    pub(super) fn collect_ids(
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

    pub(super) fn group_totals(&self, section: &IndexedSection<'_>) -> Vec<Option<f64>> {
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

    pub(super) fn ordered_grid_folds<'a>(
        &'a self,
        section: &IndexedSection<'_>,
    ) -> Vec<&'a GridFold> {
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

    pub(super) const fn fold_count(&self) -> usize {
        match &self.folds {
            FoldArena::Ranking { folds, .. } => folds.len(),
            FoldArena::Grid { folds, .. } => folds.len(),
        }
    }

    pub(super) fn for_each_fold(&self, mut visit: impl FnMut(EntityId, &Obs)) {
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

    pub(super) fn label_cutoff(&self) -> Option<LabelCutoff> {
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

    pub(super) fn score(&self, entity: EntityId, window: &Obs) -> Option<f64> {
        self.rss_mean
            .as_ref()
            .map_or_else(|| window.total(self.cumulative), |mean| mean.mean(entity))
    }

    pub(super) const fn summary(&self) -> HeatmapSummary {
        if self.rss_mean.is_some() {
            HeatmapSummary::Mean
        } else if self.cumulative {
            HeatmapSummary::Sum
        } else {
            HeatmapSummary::Max
        }
    }

    pub(super) const fn additive_summary(&self) -> bool {
        self.cumulative || self.rss_mean.is_some()
    }

    pub(super) fn finish(
        mut self,
        spec: &ItemSpec,
        dictionary: &RenderedIds,
        section: &IndexedSection<'_>,
    ) -> Result<HeatmapItemResult, HeatmapError> {
        self.close_edges();
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
    pub(super) fn finish_ungrouped(
        self,
        spec: &ItemSpec,
        mut ranked: Vec<RankedState>,
        entity_count: u64,
        totals_total: Option<f64>,
        coverage: HeatmapCoverage,
        ranking: crate::heatmap::query::NormalizedRanking,
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
                for (sum, value) in winner_sums.iter_mut().zip(grid.cells.values()) {
                    if let Some(value) = value {
                        sum.add(value);
                    }
                }
            }
            let shared = &section.entities[row.entity.index()];
            let locator = shared.locator.as_ref().ok_or_else(|| {
                HeatmapError::invalid(self.first_index, "ranked entity has no in-range locator")
            })?;
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
                    locator.segment_id,
                    locator.timestamp,
                    shared.type_id,
                    locator.ordinal,
                    locator.identity.clone(),
                ),
                total: row.total,
                cells: row.grid.map(|grid| grid.cells.values().collect()),
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

    pub(super) fn finish_grouped(
        mut self,
        spec: &ItemSpec,
        coverage: HeatmapCoverage,
        ranking: crate::heatmap::query::NormalizedRanking,
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
            for (sum, value) in group_cells[group].iter_mut().zip(state.cells.values()) {
                if let Some(value) = value {
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

fn edge_slot(samples: &mut Vec<Option<(i64, f64)>>, index: usize) -> &mut Option<(i64, f64)> {
    if samples.len() <= index {
        samples.resize(index + 1, None);
    }
    &mut samples[index]
}
