//! Bounded snapshot ranking and exact page ordering.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{ColumnClass, ColumnType};

use super::{
    CounterReadings, IdentityCell, OrderedNumber, PageContext, PageOrder, PageOrderKind,
    PageOrderValue, PageRankedRow, PageRows, PageStagedRow, PreparedSnapshot, ProcessUsers,
    SNAPSHOT_CHUNK_ROWS, SnapshotCursor, cgroup, counter_delta, identity_of, ordered_cell,
};

use crate::projection::{Plan, resolved_dictionary};
use crate::snapshot::filter::{result_search_matches, search_clause_matches};
use crate::snapshot::search::SearchValue;
use crate::statement_scope::CollectorStatements;
use crate::{Order, QueryError};

impl PreparedSnapshot {
    pub(super) fn cursor_anchor(
        &self,
        contexts: &[PageContext<'_>],
        process_users: &HashMap<usize, ProcessUsers>,
        cursor: SnapshotCursor,
    ) -> Result<PageRankedRow, QueryError> {
        let context = contexts
            .iter()
            .find(|context| context.context_index == cursor.context_index)
            .ok_or(QueryError::BadCursor)?;
        if cursor.ordinal >= context.rows {
            return Err(QueryError::BadCursor);
        }
        let source = self.dataset.open(context.source)?;
        let mut stored = None;
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            cursor.ordinal,
            1,
            |ordinal, row| {
                stored = Some((ordinal, row));
                false
            },
        )?;
        let (ordinal, row) = stored.ok_or(QueryError::BadCursor)?;
        let dictionary = page_dictionary(
            &source,
            context,
            std::slice::from_ref(&(ordinal, row.clone())),
        )?;
        self.page_candidate(
            context,
            process_users
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?,
            ordinal,
            row,
            &dictionary,
        )
        .ok_or(QueryError::BadCursor)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one scan keeps page, counters and scope explicit"
    )]
    pub(super) fn scan_page(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let source = self.dataset.open(context.source)?;
        // The collector's own statements are classified once per layout.
        let collector = if self.scope.filters(context.logical_name) {
            Some(CollectorStatements::scan(&source)?)
        } else {
            None
        };
        if !context.plan.needs_selection_dictionary()
            && context.search_columns.is_empty()
            && context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan))
                .is_none()
        {
            #[cfg(test)]
            PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
            let dictionary = Dictionary::default();
            source.visit_rows(
                context.plan.type_id,
                &context.plan.projection,
                0,
                usize::MAX,
                |ordinal, row| {
                    self.rank_page_row(
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        excluded,
                        collector.as_ref(),
                        &dictionary,
                        ordinal,
                        row,
                    );
                    !cancelled()
                },
            )?;
            return Ok(());
        }
        let mut chunk = Vec::with_capacity(SNAPSHOT_CHUNK_ROWS);
        #[cfg(test)]
        PAGE_CHUNK_ROWS.set(SNAPSHOT_CHUNK_ROWS);
        let mut failure = None;
        #[cfg(test)]
        PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
        source.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            0,
            usize::MAX,
            |ordinal, row| {
                chunk.push((ordinal, row));
                if chunk.len() == SNAPSHOT_CHUNK_ROWS
                    && let Err(error) = self.rank_page_chunk(
                        &source,
                        context,
                        process_users,
                        anchor,
                        page,
                        eligible,
                        excluded,
                        collector.as_ref(),
                        &mut chunk,
                    )
                {
                    failure = Some(error);
                    return false;
                }
                !cancelled()
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        if !cancelled() && !chunk.is_empty() {
            self.rank_page_chunk(
                &source,
                context,
                process_users,
                anchor,
                page,
                eligible,
                excluded,
                collector.as_ref(),
                &mut chunk,
            )?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "page ranking keeps segment-local user resolution beside existing paging state"
    )]
    pub(super) fn rank_page_chunk(
        &self,
        source: &Segment,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        collector: Option<&CollectorStatements>,
        chunk: &mut Vec<(u64, Row)>,
    ) -> Result<(), QueryError> {
        let dictionary = page_dictionary(source, context, chunk)?;
        for (ordinal, row) in chunk.drain(..) {
            self.rank_page_row(
                context,
                process_users,
                anchor,
                page,
                eligible,
                excluded,
                collector,
                &dictionary,
                ordinal,
                row,
            );
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "ranking keeps request, page, and physical-row coordinates explicit"
    )]
    pub(super) fn rank_page_row(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        anchor: Option<&PageRankedRow>,
        page: &mut PageRows,
        eligible: &mut u64,
        excluded: &mut u64,
        collector: Option<&CollectorStatements>,
        dictionary: &Dictionary,
        ordinal: u64,
        row: Row,
    ) {
        let Some(candidate) = self.page_candidate(context, process_users, ordinal, row, dictionary)
        else {
            return;
        };
        // A collector statement that would otherwise be listed is counted, not shown.
        if collector.is_some_and(|statements| statements.excludes(&candidate.staged.row)) {
            *excluded = excluded.saturating_add(1);
            return;
        }
        *eligible = eligible.saturating_add(1);
        if anchor.is_none_or(|anchor| candidate.cmp(anchor) != Ordering::Greater) {
            page.push(candidate);
        }
    }

    pub(super) fn page_candidate(
        &self,
        context: &PageContext<'_>,
        process_users: &ProcessUsers,
        ordinal: u64,
        row: Row,
        dictionary: &Dictionary,
    ) -> Option<PageRankedRow> {
        if !context.window.matches(&row) || !context.plan.matches(&row, dictionary) {
            return None;
        }
        let identity = identity_of(context.plan, &row)?;
        if self.search.as_ref().is_some_and(|search| {
            !search.matches_all(|clause| {
                if matches!(clause.value, SearchValue::Quantity(_)) {
                    result_search_matches(context, &row, &identity, clause)
                } else {
                    search_clause_matches(
                        context.logical_name,
                        context.plan,
                        &row,
                        dictionary,
                        Some(process_users),
                        clause,
                    )
                }
            })
        }) {
            return None;
        }
        let value = page_order_value(context, &row, &identity, dictionary);
        Some(PageRankedRow {
            staged: PageStagedRow {
                context_index: context.context_index,
                ordinal,
                row,
                identity,
            },
            value,
            direction: self.direction,
        })
    }
}

impl PageOrder {
    pub(super) fn columns(&self) -> Vec<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column) | PageOrderKind::CounterDelta(column) => vec![*column],
            PageOrderKind::CounterRatio {
                numerator,
                denominator,
                ..
            }
            | PageOrderKind::ValueRatio {
                numerator,
                denominator,
                ..
            } => numerator.iter().chain(denominator).copied().collect(),
        }
    }

    pub(super) fn dictionary_column(&self, plan: &Plan) -> Option<&'static str> {
        match &self.kind {
            PageOrderKind::Column(column)
                if plan
                    .contract
                    .column(column)
                    .is_some_and(|column| column.ty == ColumnType::StrId) =>
            {
                Some(*column)
            }
            PageOrderKind::Column(_)
            | PageOrderKind::CounterDelta(_)
            | PageOrderKind::CounterRatio { .. }
            | PageOrderKind::ValueRatio { .. } => None,
        }
    }
}

pub(super) fn page_order(
    logical_name: &str,
    plan: &Plan,
    requested: &[String],
) -> Option<PageOrder> {
    requested.iter().find_map(|name| {
        plan.contract
            .column(name)
            .map(|column| PageOrder {
                name: column.name,
                kind: PageOrderKind::Column(column.name),
            })
            .or_else(|| derived_page_order(logical_name, plan, name))
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "all fixed derived sort tokens remain visibly allowlisted together"
)]
fn derived_page_order(logical_name: &str, plan: &Plan, token: &str) -> Option<PageOrder> {
    if let Some(order) = cgroup::page_order(logical_name, plan, token) {
        return Some(order);
    }
    let supported = match logical_name {
        "pg_stat_statements" => matches!(plan.type_id, 1_002_001..=1_002_006),
        "pg_store_plans" => matches!(plan.type_id, 1_003_001 | 1_004_001 | 1_018_001),
        "pg_stat_user_tables" => matches!(plan.type_id, 1_013_005..=1_013_008),
        "pg_stat_user_indexes" => matches!(plan.type_id, 1_014_003..=1_014_004),
        _ => false,
    };
    if !supported {
        return None;
    }
    let column = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| plan.contract.column(name).map(|column| column.name))
    };
    let columns = |names: &[&str]| {
        names
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect::<Vec<_>>()
    };
    let counters =
        |name, numerator: Vec<&'static str>, denominator: Vec<&'static str>, neutral_nulls| {
            (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
                name,
                kind: PageOrderKind::CounterRatio {
                    numerator,
                    denominator,
                    neutral_nulls,
                },
            })
        };
    let one_over = |name, numerator: &[&str], denominator: &[&str]| {
        counters(
            name,
            vec![column(numerator)?],
            vec![column(denominator)?],
            false,
        )
    };
    let gauges = |name, numerator: &[&str], denominator: &[&str]| {
        let numerator = columns(numerator);
        let denominator = columns(denominator);
        (!numerator.is_empty() && !denominator.is_empty()).then_some(PageOrder {
            name,
            kind: PageOrderKind::ValueRatio {
                numerator,
                denominator,
            },
        })
    };
    match token {
        "derived.mean_exec_ms_per_call" => one_over(
            "mean_exec_ms_per_call",
            &["total_exec_time", "total_time"],
            &["calls"],
        ),
        "derived.rows_per_call" => one_over("rows_per_call", &["rows"], &["calls"]),
        "derived.blocks_per_call" => counters(
            "blocks_per_call",
            [
                "shared_blks_hit",
                "shared_blks_read",
                "local_blks_hit",
                "local_blks_read",
            ]
            .iter()
            .filter_map(|name| column(&[*name]))
            .collect(),
            vec![column(&["calls"])?],
            false,
        ),
        "derived.hit_pct" => {
            let hit = column(&["shared_blks_hit"])?;
            let read = column(&["shared_blks_read"])?;
            counters("hit_pct", vec![hit], vec![hit, read], false)
        }
        "derived.wal_per_call" => one_over("wal_per_call", &["wal_bytes"], &["calls"]),
        "derived.plan_time_pct" => {
            let planning = column(&["total_plan_time"])?;
            let execution = column(&["total_exec_time", "total_time"])?;
            counters(
                "plan_time_pct",
                vec![planning],
                vec![planning, execution],
                false,
            )
        }
        "derived.cv" => Some(PageOrder {
            name: "cv",
            kind: PageOrderKind::ValueRatio {
                numerator: vec![column(&["stddev_exec_time", "stddev_time"])?],
                denominator: vec![column(&["mean_exec_time", "mean_time"])?],
            },
        }),
        "derived.dead_pct" => gauges("dead_pct", &["n_dead_tup"], &["n_live_tup", "n_dead_tup"]),
        "derived.hot_pct" => one_over("hot_pct", &["n_tup_hot_upd"], &["n_tup_upd"]),
        "derived.new_page_pct" => one_over("new_page_pct", &["n_tup_newpage_upd"], &["n_tup_upd"]),
        "derived.sequential_share_pct" => counters(
            "sequential_share_pct",
            columns(&["seq_scan"]),
            columns(&["seq_scan", "idx_scan"]),
            true,
        ),
        "derived.seq_tuples_per_scan" => {
            one_over("seq_tuples_per_scan", &["seq_tup_read"], &["seq_scan"])
        }
        "derived.idx_tuples_per_scan" => {
            one_over("idx_tuples_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        "derived.buffer_hit_pct" => counters(
            "buffer_hit_pct",
            columns(&[
                "heap_blks_hit",
                "idx_blks_hit",
                "toast_blks_hit",
                "tidx_blks_hit",
            ]),
            columns(&[
                "heap_blks_hit",
                "heap_blks_read",
                "idx_blks_hit",
                "idx_blks_read",
                "toast_blks_hit",
                "toast_blks_read",
                "tidx_blks_hit",
                "tidx_blks_read",
            ]),
            true,
        ),
        "derived.vacuum_mean_ms" => {
            one_over("vacuum_mean_ms", &["total_vacuum_time"], &["vacuum_count"])
        }
        "derived.autovacuum_mean_ms" => one_over(
            "autovacuum_mean_ms",
            &["total_autovacuum_time"],
            &["autovacuum_count"],
        ),
        "derived.analyze_mean_ms" => one_over(
            "analyze_mean_ms",
            &["total_analyze_time"],
            &["analyze_count"],
        ),
        "derived.autoanalyze_mean_ms" => one_over(
            "autoanalyze_mean_ms",
            &["total_autoanalyze_time"],
            &["autoanalyze_count"],
        ),
        "derived.tuples_per_scan" => one_over("tuples_per_scan", &["idx_tup_read"], &["idx_scan"]),
        "derived.fetches_per_scan" => {
            one_over("fetches_per_scan", &["idx_tup_fetch"], &["idx_scan"])
        }
        _ => None,
    }
}
impl PageRows {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            limit,
            rows: BinaryHeap::new(),
        }
    }

    pub(super) fn push(&mut self, row: PageRankedRow) {
        if self.limit == 0 {
            return;
        }
        if self.rows.len() < self.limit {
            self.rows.push(Reverse(row));
            return;
        }
        let Some(worst) = self.rows.peek() else {
            return;
        };
        if row > worst.0 {
            self.rows.pop();
            self.rows.push(Reverse(row));
        }
    }

    pub(super) fn finish(self) -> Vec<PageRankedRow> {
        let mut rows: Vec<PageRankedRow> = self.rows.into_iter().map(|Reverse(row)| row).collect();
        rows.sort_by(|left, right| right.cmp(left));
        rows
    }

    #[cfg(test)]
    pub(super) fn retained_len(&self) -> usize {
        self.rows.len()
    }
}

impl PartialEq for PageRankedRow {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PageRankedRow {}

impl PartialOrd for PageRankedRow {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PageRankedRow {
    fn cmp(&self, other: &Self) -> Ordering {
        debug_assert_eq!(
            self.direction, other.direction,
            "one page heap cannot compare opposite sort directions"
        );
        compare_page_order_values(self.value.as_ref(), other.value.as_ref(), self.direction)
            .then_with(|| other.staged.context_index.cmp(&self.staged.context_index))
            .then_with(|| other.staged.ordinal.cmp(&self.staged.ordinal))
    }
}

pub(super) fn compare_page_order_values(
    left: Option<&PageOrderValue>,
    right: Option<&PageOrderValue>,
    direction: Order,
) -> Ordering {
    let ordered = match (left, right) {
        (Some(PageOrderValue::Integer(left)), Some(PageOrderValue::Integer(right))) => {
            left.cmp(right)
        }
        (Some(PageOrderValue::Float(left)), Some(PageOrderValue::Float(right)))
        | (Some(PageOrderValue::FloatRate(left)), Some(PageOrderValue::FloatRate(right)))
        | (Some(PageOrderValue::FloatRatio(left)), Some(PageOrderValue::FloatRatio(right))) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (
            Some(PageOrderValue::IntegerRate {
                delta: left,
                elapsed: left_elapsed,
            }),
            Some(PageOrderValue::IntegerRate {
                delta: right,
                elapsed: right_elapsed,
            }),
        ) => (left * i128::from(*right_elapsed)).cmp(&(right * i128::from(*left_elapsed))),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator: left_numerator,
                denominator: left_denominator,
            }),
            Some(PageOrderValue::IntegerRatio {
                numerator: right_numerator,
                denominator: right_denominator,
            }),
        ) => compare_u128_ratios(
            *left_numerator,
            *left_denominator,
            *right_numerator,
            *right_denominator,
        ),
        (
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
            Some(PageOrderValue::FloatRatio(right)),
        ) => integer_ratio_as_f64(*numerator, *denominator)
            .partial_cmp(right)
            .unwrap_or(Ordering::Equal),
        (
            Some(PageOrderValue::FloatRatio(left)),
            Some(PageOrderValue::IntegerRatio {
                numerator,
                denominator,
            }),
        ) => left
            .partial_cmp(&integer_ratio_as_f64(*numerator, *denominator))
            .unwrap_or(Ordering::Equal),
        (Some(PageOrderValue::Text(left)), Some(PageOrderValue::Text(right))) => left.cmp(right),
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
    };
    match direction {
        Order::Asc => ordered.reverse(),
        Order::Desc => ordered,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "this path compares an exact ratio with an already inexact floating source"
)]
fn integer_ratio_as_f64(numerator: u128, denominator: u128) -> f64 {
    numerator as f64 / denominator as f64
}

pub(super) fn compare_u128_ratios(
    mut left_numerator: u128,
    mut left_denominator: u128,
    mut right_numerator: u128,
    mut right_denominator: u128,
) -> Ordering {
    let mut reverse = false;
    loop {
        let whole = (left_numerator / left_denominator).cmp(&(right_numerator / right_denominator));
        if whole != Ordering::Equal {
            return if reverse { whole.reverse() } else { whole };
        }
        let left_remainder = left_numerator % left_denominator;
        let right_remainder = right_numerator % right_denominator;
        let ended = match (left_remainder == 0, right_remainder == 0) {
            (true, true) => return Ordering::Equal,
            (true, false) => Some(Ordering::Less),
            (false, true) => Some(Ordering::Greater),
            (false, false) => None,
        };
        if let Some(ended) = ended {
            return if reverse { ended.reverse() } else { ended };
        }
        (left_numerator, left_denominator) = (left_denominator, left_remainder);
        (right_numerator, right_denominator) = (right_denominator, right_remainder);
        reverse = !reverse;
    }
}

#[cfg(test)]
pub(super) fn compare_ordered(
    left: Option<OrderedNumber>,
    right: Option<OrderedNumber>,
) -> Ordering {
    match (left, right) {
        (Some(OrderedNumber::Integer(left)), Some(OrderedNumber::Integer(right))) => {
            left.cmp(&right)
        }
        (Some(OrderedNumber::Float(left)), Some(OrderedNumber::Float(right))) => {
            left.partial_cmp(&right).unwrap_or(Ordering::Equal)
        }
        (Some(_), Some(_)) | (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
    }
}
fn page_dictionary(
    segment: &Segment,
    context: &PageContext<'_>,
    rows: &[(u64, Row)],
) -> Result<Dictionary, QueryError> {
    let mut ids = HashSet::new();
    for (_ordinal, row) in rows {
        if !context.window.matches(row) {
            continue;
        }
        context.plan.add_selection_ids(row, &mut ids);
        for column in context.search_columns.iter().copied().chain(
            context
                .order
                .as_ref()
                .and_then(|order| order.dictionary_column(context.plan)),
        ) {
            if let Some(Cell::StrId(id)) = row.get(column) {
                ids.insert(*id);
            }
        }
    }
    #[cfg(test)]
    if !ids.is_empty() {
        PAGE_CANDIDATE_DICTIONARIES.set(PAGE_CANDIDATE_DICTIONARIES.get() + 1);
    }
    resolved_dictionary(segment, &ids)
}

fn page_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
) -> Option<PageOrderValue> {
    let order = context.order.as_ref()?;
    if order.name == "quota_cores"
        && !matches!(row.get("quota_usec"), Some(Cell::I64(value)) if *value > 0)
    {
        return None;
    }
    match &order.kind {
        PageOrderKind::CounterDelta(column) => {
            let _elapsed = context.elapsed_for(row)?;
            let before = context.predecessor(row, identity)?;
            match counter_delta(row.get(column)?, before.get(column)?)? {
                OrderedNumber::Integer(delta) => Some(PageOrderValue::Integer(delta)),
                OrderedNumber::Float(delta) => Some(PageOrderValue::Float(delta)),
            }
        }
        PageOrderKind::Column(column) => {
            column_order_value(context, row, identity, dictionary, column)
        }
        PageOrderKind::CounterRatio {
            numerator,
            denominator,
            neutral_nulls,
        } => {
            let _elapsed = context.elapsed_for(row)?;
            let before = context.predecessor(row, identity)?;
            let numerator = counter_sum(row, before, numerator, *neutral_nulls)?;
            let denominator = counter_sum(row, before, denominator, *neutral_nulls)?;
            ratio_order_value(numerator, denominator)
        }
        PageOrderKind::ValueRatio {
            numerator,
            denominator,
        } => ratio_order_value(value_sum(row, numerator)?, value_sum(row, denominator)?),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "an interval of 2^52 microseconds is 142 years"
)]
fn column_order_value(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    dictionary: &Dictionary,
    column: &'static str,
) -> Option<PageOrderValue> {
    let stored = row.get(column)?;
    let cumulative = context
        .plan
        .contract
        .column(column)
        .is_some_and(|declared| declared.class == ColumnClass::Cumulative);
    if cumulative {
        let elapsed = context.elapsed_for(row)?;
        let earlier = context.predecessor(row, identity)?.get(column)?;
        return match counter_delta(stored, earlier)? {
            OrderedNumber::Integer(delta) => Some(PageOrderValue::IntegerRate { delta, elapsed }),
            OrderedNumber::Float(delta) => {
                let seconds = elapsed as f64 / 1_000_000.0;
                let rate = delta / seconds;
                rate.is_finite().then_some(PageOrderValue::FloatRate(rate))
            }
        };
    }
    match stored {
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .map(Resolved::stored_bytes)
            .map(<[u8]>::to_vec)
            .map(PageOrderValue::Text),
        _ => match ordered_cell(stored)? {
            OrderedNumber::Integer(value) => Some(PageOrderValue::Integer(value)),
            OrderedNumber::Float(value) => Some(PageOrderValue::Float(value)),
        },
    }
}

pub(super) fn counter_sum(
    row: &Row,
    before: &CounterReadings,
    columns: &[&'static str],
    neutral_nulls: bool,
) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let (Some(now), Some(earlier)) = (row.get(column), before.get(column)) else {
            return None;
        };
        if neutral_nulls && matches!((now, earlier), (Cell::Null, Cell::Null)) {
            continue;
        }
        let value = counter_delta(now, earlier)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

pub(super) fn value_sum(row: &Row, columns: &[&'static str]) -> Option<OrderedNumber> {
    let mut sum = None;
    for column in columns {
        let value = row.get(column).and_then(ordered_cell)?;
        sum = Some(add_ordered(sum, value)?);
    }
    sum
}

fn add_ordered(left: Option<OrderedNumber>, right: OrderedNumber) -> Option<OrderedNumber> {
    match (left, right) {
        (None, right) => Some(right),
        (Some(OrderedNumber::Integer(left)), OrderedNumber::Integer(right)) => {
            left.checked_add(right).map(OrderedNumber::Integer)
        }
        (Some(left), right) => {
            let sum = left.as_f64() + right.as_f64();
            sum.is_finite().then_some(OrderedNumber::Float(sum))
        }
    }
}

fn ratio_order_value(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
) -> Option<PageOrderValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(PageOrderValue::IntegerRatio {
                numerator: u128::try_from(numerator).ok()?,
                denominator: u128::try_from(denominator).ok()?,
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator;
            (denominator > 0.0 && ratio.is_finite()).then_some(PageOrderValue::FloatRatio(ratio))
        }
    }
}

#[cfg(test)]
use super::{PAGE_CANDIDATE_DICTIONARIES, PAGE_CHUNK_ROWS, PAGE_SOURCE_VISITS};
