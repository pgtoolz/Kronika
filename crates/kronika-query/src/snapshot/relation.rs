//! Fixed relation views for `PostgreSQL` table and index snapshots.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Row};
use serde_json::{Map, Value, json};

use super::{
    CounterReadings, PageContext, Plan, PreparedSnapshot, SearchClause, SearchOperator,
    SearchValue, SectionPlans, SnapshotCursor, StructuredSearch, identity_of, result_field,
    search_fields, search_matches, search_value_matches, validate_search_projection,
};
use crate::render::record;
use crate::{
    Filter, GroupKey, Metric, Order, QueryError, RelationAggregate, RelationGroup, RelationKind,
    RelationSource, SnapshotRequest, index_scan_rate_is_zero, key_fields, resolved_dictionary,
};

const INDEXES: &str = "pg_stat_user_indexes";

pub(super) fn snapshot_physical_fields(
    logical_name: &str,
    group: RelationGroup,
    fields: &[String],
    by: &[String],
    search: Option<&StructuredSearch>,
) -> Result<Vec<String>, QueryError> {
    let kind = RelationKind::from_name(logical_name)?;
    let mut semantic = fields.to_vec();
    semantic.extend(by.iter().map(|name| sort_name(name).to_owned()));
    let mut names = kind
        .physical_fields(group, &semantic)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    names.extend(
        key_fields(kind, RelationGroup::Object)
            .iter()
            .map(|name| (*name).to_owned()),
    );
    if let Some(search) = search {
        for (_clause, field) in search.result_clauses(logical_name) {
            names.extend(field.dependencies.iter().map(|name| (*name).to_owned()));
        }
    }
    Ok(names.into_iter().collect())
}

pub(super) fn split_filters(
    request: &SnapshotRequest,
) -> Result<(Vec<Filter>, Vec<Filter>), QueryError> {
    if request.group.is_none() {
        return Ok((request.filters.clone(), Vec::new()));
    }
    let [section] = request.sections.as_slice() else {
        return Err(QueryError::BadFilter("where".to_owned()));
    };
    let mut physical = Vec::new();
    let mut derived = Vec::new();
    for filter in &request.filters {
        if filter.column == "tablespace_oid"
            && filter.value.parse::<u32>().ok().is_none_or(|oid| oid == 0)
        {
            return Err(QueryError::BadFilter(filter.column.clone()));
        }
        if filter.column == "no_scans" {
            if section != INDEXES || filter.value != "true" {
                return Err(QueryError::BadFilter(filter.column.clone()));
            }
            derived.push(filter.clone());
        } else {
            physical.push(filter.clone());
        }
    }
    Ok((physical, derived))
}

/// One typed grouped-relation result used by non-streaming adapters.
#[derive(Debug)]
pub struct RelationRow {
    /// Stable grouping identity.
    pub key: GroupKey,
    /// Requested semantic metrics keyed by public field name.
    pub metrics: BTreeMap<String, Option<Metric>>,
    source: RelationSource,
    from: Option<i64>,
    to: Option<i64>,
}

impl PreparedSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "one streaming routine preserves the exact aggregate-sort-page-cursor pipeline"
    )]
    pub(super) fn emit_relation_page(
        &self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let group = self.group.ok_or(QueryError::BadCursor)?;
        let kind = RelationKind::from_name(&section.logical_name)?;
        let fields = kind.fields(group);
        let keys = key_fields(kind, group);
        for name in &self.by {
            let semantic = sort_name(name);
            if !fields.iter().any(|field| field.name() == semantic) && !keys.contains(&semantic) {
                return Err(QueryError::NoSuchColumn(name.clone()));
            }
        }
        if cancelled()
            || !emit(relation_layout(
                section,
                kind,
                group,
                &self.relation_fields,
            )?)
        {
            return Ok(());
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let mut aggregates = BTreeMap::<GroupKey, RelationAggregate>::new();
        for context in &contexts {
            scan_context(self, kind, group, context, &mut aggregates, cancelled)?;
            if cancelled() {
                return Ok(());
            }
        }
        aggregates.retain(|_key, aggregate| {
            matches_result_search(aggregate, kind, group, self.search.as_deref())
        });
        let eligible = u64::try_from(aggregates.len()).unwrap_or(u64::MAX);
        let order_by = self.by.first().map(|name| sort_name(name));
        let mut ranked = aggregates
            .into_values()
            .map(|aggregate| {
                let sort = order_by.and_then(|name| {
                    aggregate
                        .metric(kind, group, name)
                        .or_else(|| aggregate.key().metric(name))
                });
                (aggregate, sort)
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left, left_sort), (right, right_sort)| {
            compare_relation_order(
                left.key(),
                left_sort.as_ref(),
                right.key(),
                right_sort.as_ref(),
                self.direction,
            )
        });
        let start = match self.cursor {
            Some(cursor) => ranked
                .iter()
                .position(|(aggregate, _sort)| {
                    aggregate.source().context_index() == cursor.context_index
                        && aggregate.source().ordinal() == cursor.ordinal
                })
                .ok_or(QueryError::BadCursor)?,
            None => 0,
        };
        let page_size = self.page_size.ok_or(QueryError::BadCursor)?;
        let end = start.saturating_add(page_size).min(ranked.len());
        let has_more = end < ranked.len();
        let next_cursor = has_more.then(|| {
            let source = ranked[end].0.source();
            SnapshotCursor {
                segment_id: self.anchor.id(),
                active_position: self.anchor.active_position().unwrap_or(0),
                context_index: source.context_index(),
                ordinal: source.ordinal(),
                binding: self.binding,
            }
            .encode()
        });
        let returned = end.saturating_sub(start);
        let from = ranked
            .iter()
            .filter_map(|(row, _sort)| row.sample_from())
            .min();
        let to = ranked
            .iter()
            .filter_map(|(row, _sort)| row.sample_to())
            .max();
        for (aggregate, _sort) in ranked.drain(start..end) {
            #[cfg(test)]
            super::RELATION_PROJECTED_METRICS.set(
                super::RELATION_PROJECTED_METRICS
                    .get()
                    .saturating_add(self.relation_fields.len()),
            );
            let metrics = self
                .relation_fields
                .iter()
                .map(|name| (name.clone(), aggregate.metric(kind, group, name)))
                .collect();
            let row = RelationRow {
                key: aggregate.key().clone(),
                metrics,
                source: aggregate.source(),
                from: aggregate.sample_from(),
                to: aggregate.sample_to(),
            };
            if cancelled()
                || !emit(relation_record(
                    section,
                    kind,
                    group,
                    &row,
                    group == RelationGroup::Object,
                )?)
            {
                return Ok(());
            }
        }
        let _connected = emit(record(json!({
            "record": "snapshot_page",
            "logical_name": section.logical_name,
            "group": group.as_str(),
            "eligible": eligible.to_string(),
            "returned": returned.to_string(),
            "has_more": has_more,
            "truncated": eligible > returned as u64,
            "next_cursor": next_cursor,
            "page_size": page_size,
            "order_by": order_by.into_iter().collect::<Vec<_>>(),
            "order_direction": self.direction.as_str(),
            "from": from.map(|value| value.to_string()),
            "to": to.map(|value| value.to_string()),
        }))?);
        Ok(())
    }

    /// Returns the first `limit` aggregates after filter and sort, plus whether
    /// more matched.
    pub(crate) fn compute_relation_rows(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<super::selector::FinderResult<RelationRow>, QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let group = self.group.ok_or(QueryError::BadCursor)?;
        let kind = RelationKind::from_name(&section.logical_name)?;
        let fields = kind.fields(group);
        let keys = key_fields(kind, group);
        for name in &self.by {
            let semantic = sort_name(name);
            if !fields.iter().any(|field| field.name() == semantic) && !keys.contains(&semantic) {
                return Err(QueryError::NoSuchColumn(name.clone()));
            }
        }
        let contexts = self.partitioned_contexts(section, cancelled)?;
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        let as_of = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let mut aggregates = BTreeMap::<GroupKey, RelationAggregate>::new();
        for context in &contexts {
            if cancelled() {
                return Err(QueryError::Cancelled);
            }
            scan_context(self, kind, group, context, &mut aggregates, cancelled)?;
        }
        aggregates.retain(|_key, aggregate| {
            matches_result_search(aggregate, kind, group, self.search.as_deref())
        });
        let order_by = self.by.first().map(|name| sort_name(name));
        let mut ranked = aggregates
            .into_values()
            .map(|aggregate| {
                let sort = order_by.and_then(|name| {
                    aggregate
                        .metric(kind, group, name)
                        .or_else(|| aggregate.key().metric(name))
                });
                (aggregate, sort)
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|(left, left_sort), (right, right_sort)| {
            compare_relation_order(
                left.key(),
                left_sort.as_ref(),
                right.key(),
                right_sort.as_ref(),
                self.direction,
            )
        });
        let truncated = ranked.len() > limit;
        let rows = ranked
            .into_iter()
            .take(limit)
            .map(|(aggregate, _sort)| {
                let metrics = self
                    .relation_fields
                    .iter()
                    .map(|name| (name.clone(), aggregate.metric(kind, group, name)))
                    .collect();
                RelationRow {
                    key: aggregate.key().clone(),
                    metrics,
                    source: aggregate.source(),
                    from: aggregate.sample_from(),
                    to: aggregate.sample_to(),
                }
            })
            .collect();
        Ok(super::selector::FinderResult {
            rows,
            truncated,
            as_of,
        })
    }

    /// Applies a typed expression after grouped-phase and projection
    /// validation. `None` leaves the snapshot unchanged.
    pub(crate) fn with_search(
        mut self,
        search: Option<StructuredSearch>,
    ) -> Result<Self, QueryError> {
        let Some(search) = search else {
            return Ok(self);
        };
        if self
            .group
            .is_some_and(|group| group != RelationGroup::Object)
        {
            search
                .validate_grouped_phase()
                .map_err(|_diagnostic| QueryError::BadFilter("search".to_owned()))?;
        }
        validate_search_projection(Some(&search), &self.sections)?;
        self.search = Some(Box::new(search));
        Ok(self)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one bounded scan preserves filter-search-delta-group ordering"
)]
fn scan_context(
    prepared: &PreparedSnapshot,
    kind: RelationKind,
    group: RelationGroup,
    context: &PageContext<'_>,
    aggregates: &mut BTreeMap<GroupKey, RelationAggregate>,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<(), QueryError> {
    let source_segment = prepared.dataset.open(context.source)?;
    let mut offset = 0_u64;
    while offset < context.rows {
        let mut chunk = Vec::new();
        source_segment.visit_rows(
            context.plan.type_id,
            &context.plan.projection,
            offset,
            super::SNAPSHOT_CHUNK_ROWS,
            |ordinal, row| {
                if cancelled() {
                    return false;
                }
                chunk.push((ordinal, row));
                true
            },
        )?;
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if chunk.is_empty() {
            break;
        }
        offset = chunk
            .last()
            .map_or(context.rows, |(ordinal, _row)| ordinal.saturating_add(1));
        let mut ids = HashSet::new();
        for (_ordinal, row) in &chunk {
            if !context.window.matches(row) {
                continue;
            }
            for (_name, cell) in row.iter() {
                if let Cell::StrId(id) = cell {
                    ids.insert(*id);
                }
            }
        }
        let dictionary = resolved_dictionary(&source_segment, &ids)?;
        for (ordinal, row) in chunk {
            if !context.window.matches(&row)
                || !context.plan.matches(&row, &dictionary)
                || (group != RelationGroup::Object
                    && prepared.search.as_ref().is_some_and(|search| {
                        !search_matches(
                            kind.logical_name(),
                            context.plan,
                            &row,
                            &dictionary,
                            None,
                            search,
                        )
                    }))
            {
                continue;
            }
            let Some(identity) = identity_of(context.plan, &row) else {
                continue;
            };
            let Some(timestamp) = context
                .plan
                .timestamp
                .and_then(|name| timestamp_cell(row.get(name)))
            else {
                continue;
            };
            let source = RelationSource::new(
                context.source.id(),
                context.context_index,
                ordinal,
                context.plan.type_id,
                timestamp,
            );
            let before = context
                .previous
                .as_ref()
                .and_then(|previous| previous.get(&identity));
            let elapsed = context.elapsed_for(&row);
            if !matches_derived_filters(
                kind,
                &prepared.relation_filters,
                context.plan,
                &row,
                before,
                elapsed,
            ) {
                continue;
            }
            let Some(key) = GroupKey::from_row(kind, group, &row, &dictionary)? else {
                continue;
            };
            aggregates
                .entry(key.clone())
                .or_insert_with(|| RelationAggregate::new(key, source))
                .add(
                    kind,
                    context.plan,
                    &row,
                    before,
                    elapsed,
                    &dictionary,
                    source,
                )?;
        }
    }
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    Ok(())
}

fn matches_derived_filters(
    kind: RelationKind,
    filters: &[Filter],
    plan: &Plan,
    row: &Row,
    before: Option<&CounterReadings>,
    elapsed: Option<i64>,
) -> bool {
    filters
        .iter()
        .all(|filter| match (kind, filter.column.as_str()) {
            (RelationKind::Indexes, "no_scans") => {
                index_scan_rate_is_zero(plan, row, before, elapsed)
            }
            _ => false,
        })
}

fn matches_result_search(
    aggregate: &RelationAggregate,
    kind: RelationKind,
    group: RelationGroup,
    search: Option<&StructuredSearch>,
) -> bool {
    search.is_none_or(|search| {
        if group == RelationGroup::Object {
            search.matches_all(|clause| matches_search_clause(aggregate, kind, group, clause))
        } else {
            search.matches_result(|clause| matches_search_clause(aggregate, kind, group, clause))
        }
    })
}

fn matches_search_clause(
    aggregate: &RelationAggregate,
    kind: RelationKind,
    group: RelationGroup,
    clause: &SearchClause,
) -> bool {
    if let SearchValue::Quantity(quantity) = &clause.value {
        let Some(result) = result_field(kind.logical_name(), clause.key) else {
            return false;
        };
        return aggregate
            .metric(kind, group, result.metric)
            .and_then(|metric| metric.compare_ratio(quantity.numerator, quantity.denominator))
            .is_some_and(|ordering| match clause.operator {
                SearchOperator::Greater => ordering == Ordering::Greater,
                SearchOperator::Less => ordering == Ordering::Less,
                SearchOperator::Colon => false,
            });
    }
    let Some(field) = search_fields(kind.logical_name())
        .iter()
        .find(|field| field.key == clause.key)
    else {
        return false;
    };
    field.columns.iter().any(|column| {
        aggregate
            .text(column)
            .or_else(|| aggregate.key().text(column))
            .is_some_and(|stored| search_value_matches(stored, &clause.value))
    })
}

fn relation_layout(
    section: &SectionPlans,
    kind: RelationKind,
    group: RelationGroup,
    selected: &[String],
) -> Result<Vec<u8>, QueryError> {
    let available = kind.fields(group);
    let columns = selected
        .iter()
        .filter_map(|name| available.iter().find(|field| field.name() == name))
        .map(|field| {
            json!({
                "name": field.name(),
                "kind": field.kind_name(),
                "unit": field.unit().unwrap_or("none"),
                "nullable": true,
            })
        })
        .collect::<Vec<_>>();
    record(json!({
        "record": "relation_layout",
        "logical_name": section.logical_name,
        "group": group.as_str(),
        "columns": columns,
    }))
}

fn relation_record(
    section: &SectionPlans,
    kind: RelationKind,
    group: RelationGroup,
    row: &RelationRow,
    physical_source: bool,
) -> Result<Vec<u8>, QueryError> {
    let values = relation_values(&row.metrics);
    let source = physical_source.then(|| {
        json!({
            "segment_id": row.source.segment_id().to_string(),
            "type_id": row.source.type_id().to_string(),
            "ordinal": row.source.ordinal().to_string(),
            "timestamp": row.source.timestamp().to_string(),
        })
    });
    record(json!({
        "record": "relation",
        "logical_name": section.logical_name,
        "group": group.as_str(),
        "key": row.key.json(kind, group),
        "values": values,
        "sample_from": row.from.map(|value| value.to_string()),
        "sample_to": row.to.map(|value| value.to_string()),
        "source": source,
    }))
}

fn relation_values(metrics: &BTreeMap<String, Option<Metric>>) -> Map<String, Value> {
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

fn compare_relation_order(
    left_key: &GroupKey,
    left_sort: Option<&Metric>,
    right_key: &GroupKey,
    right_sort: Option<&Metric>,
    direction: Order,
) -> Ordering {
    let ordered = match (left_sort, right_sort) {
        (Some(left), Some(right)) => left.compare(right).unwrap_or(Ordering::Equal),
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    let ordered = match direction {
        Order::Asc => ordered,
        Order::Desc => ordered.reverse(),
    };
    ordered.then_with(|| left_key.cmp(right_key))
}

const fn timestamp_cell(stored: Option<&Cell>) -> Option<i64> {
    match stored {
        Some(Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

fn sort_name(name: &str) -> &str {
    name.strip_prefix("derived.").unwrap_or(name)
}
