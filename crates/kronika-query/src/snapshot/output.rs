//! Typed finder rows and their exact physical locators.

use std::collections::{BTreeMap, HashMap, HashSet};

use kronika_reader::Cell;
use kronika_registry::logical_section_name;
use serde_json::{Value, json};

use super::{
    PageContext, PageRankedRow, PageRows, PlainRowOut, PreparedSnapshot, ProcessRowOut,
    ProcessUsers, RankedLocatorKey, RankedRecord, RowCoordinate, RowLocator,
    encoded_locator_identity, non_unique_locator,
};

use crate::QueryError;
use crate::projection::{Plan, resolved_dictionary};
use crate::snapshot::selector::FinderResult;

impl PreparedSnapshot {
    /// Returns the first `limit` rows from the full sort and whether more
    /// matched. Supports `os_process` only.
    pub(crate) fn compute_process_rows(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<FinderResult<ProcessRowOut>, QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        if section.logical_name != "os_process" {
            return Err(QueryError::NoSuchSection);
        }
        let (records, truncated, as_of) = self.ranked_records(limit, cancelled)?;
        let rows = records
            .into_iter()
            .map(|(plan, record, identity)| process_row_out(plan, record, identity))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FinderResult {
            rows,
            truncated,
            as_of,
        })
    }

    /// Returns the first `limit` rows from the full sort for supported
    /// plain (non-relation) sections.
    pub(crate) fn compute_plain_rows(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<FinderResult<PlainRowOut>, QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        if !matches!(
            section.logical_name.as_str(),
            "pg_stat_activity"
                | "pg_locks"
                | "pg_stat_progress_vacuum"
                | "pg_stat_database"
                | "pg_stat_statements"
                | "pg_store_plans"
                | "pg_settings"
                | "instance_metadata"
        ) {
            return Err(QueryError::NoSuchSection);
        }
        let (records, truncated, as_of) = self.ranked_records(limit, cancelled)?;
        let rows = records
            .into_iter()
            .map(|(plan, record, identity)| plain_row_out(plan, record, identity))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FinderResult {
            rows,
            truncated,
            as_of,
        })
    }

    /// Ranks one section from the start and renders survivors through
    /// `row_record`. Callers validate the section name.
    pub(super) fn ranked_records(
        &self,
        limit: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(Vec<RankedRecord<'_>>, bool, Option<i64>), QueryError> {
        let [section] = self.sections.as_slice() else {
            return Err(QueryError::BadCursor);
        };
        let contexts = self.page_contexts(section, cancelled)?;
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        let as_of = contexts
            .iter()
            .filter_map(|context| context.sample_to)
            .max();
        let process_users = contexts
            .iter()
            .map(|context| {
                let source = self.dataset.open(context.source)?;
                ProcessUsers::load(&source, context.plan)
                    .map(|users| (context.context_index, users))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut page = PageRows::new(limit.saturating_add(1));
        let mut eligible = 0_u64;
        let mut excluded = 0_u64;
        for context in &contexts {
            self.scan_page(
                context,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                None,
                &mut page,
                &mut eligible,
                &mut excluded,
                cancelled,
            )?;
            if cancelled() {
                return Err(QueryError::Cancelled);
            }
        }
        let mut ranked = page.finish();
        let has_more = ranked.len() > limit;
        ranked.truncate(limit);
        self.validate_ranked_locator_identities(&contexts, &ranked, cancelled)?;
        let records = self.render_ranked_rows(&contexts, &process_users, ranked)?;
        Ok((records, has_more, as_of))
    }

    /// Proves that every bounded finder survivor names one physical row.
    /// A second minimal projection keeps top-K retention bounded while finding
    /// a duplicate that appeared anywhere in the selected snapshot.
    pub(super) fn validate_ranked_locator_identities(
        &self,
        contexts: &[PageContext<'_>],
        ranked: &[PageRankedRow],
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), QueryError> {
        let mut targets: HashMap<RankedLocatorKey, u8> = HashMap::with_capacity(ranked.len());
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let (at, identity) = encoded_locator_identity(context.plan, &ranked.staged.row)?;
            if targets
                .insert((context.context_index, at, identity), 0)
                .is_some()
            {
                return Err(non_unique_locator(context, at));
            }
        }
        for context in contexts {
            if !targets
                .keys()
                .any(|(context_index, _at, _identity)| *context_index == context.context_index)
            {
                continue;
            }
            let timestamp = context.plan.timestamp.ok_or(QueryError::BadCursor)?;
            let mut projection = vec![timestamp];
            projection.extend(crate::identity_columns(context.plan.contract));
            let source = self.dataset.open(context.source)?;
            let mut failure = None;
            #[cfg(test)]
            PAGE_SOURCE_VISITS.set(PAGE_SOURCE_VISITS.get() + 1);
            source.visit_rows(
                context.plan.type_id,
                &projection,
                0,
                usize::MAX,
                |_ordinal, row| {
                    if cancelled() {
                        return false;
                    }
                    if !context.window.matches(&row) {
                        return true;
                    }
                    let (at, identity) = match encoded_locator_identity(context.plan, &row) {
                        Ok(key) => key,
                        Err(error) => {
                            failure = Some(error);
                            return false;
                        }
                    };
                    let Some(count) = targets.get_mut(&(context.context_index, at, identity))
                    else {
                        return true;
                    };
                    *count = count.saturating_add(1);
                    if *count > 1 {
                        failure = Some(non_unique_locator(context, at));
                        return false;
                    }
                    true
                },
            )?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if targets.values().all(|count| *count == 1) {
            return Ok(());
        }
        Err(QueryError::BadLocator(
            "cannot emit detail_locator: a selected row identity was not found".to_owned(),
        ))
    }

    /// Renders ranked rows with the `Plan` needed to re-key positional fields.
    pub(super) fn render_ranked_rows<'a>(
        &self,
        contexts: &[PageContext<'a>],
        process_users: &HashMap<usize, ProcessUsers>,
        ranked: Vec<PageRankedRow>,
    ) -> Result<Vec<RankedRecord<'a>>, QueryError> {
        let mut ids_by_context: HashMap<usize, HashSet<u64>> = HashMap::new();
        for ranked in &ranked {
            for (_name, value) in ranked.staged.row.iter() {
                if let Cell::StrId(id) = value {
                    ids_by_context
                        .entry(ranked.staged.context_index)
                        .or_default()
                        .insert(*id);
                }
            }
        }
        let dictionaries = contexts
            .iter()
            .map(|context| {
                let ids = ids_by_context
                    .get(&context.context_index)
                    .cloned()
                    .unwrap_or_default();
                let source = self.dataset.open(context.source)?;
                resolved_dictionary(&source, &ids)
                    .map(|dictionary| (context.context_index, dictionary))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut records = Vec::with_capacity(ranked.len());
        for ranked in ranked {
            let context = contexts
                .iter()
                .find(|context| context.context_index == ranked.staged.context_index)
                .ok_or(QueryError::BadCursor)?;
            let dictionary = dictionaries
                .get(&context.context_index)
                .ok_or(QueryError::BadCursor)?;
            let before = context.predecessor(&ranked.staged.row, &ranked.staged.identity);
            let record = Self::row_record(
                context.plan,
                &ranked.staged.row,
                before,
                context.elapsed_for(&ranked.staged.row),
                RowCoordinate {
                    segment_id: context.source.id(),
                    ordinal: ranked.staged.ordinal,
                },
                dictionary,
                process_users
                    .get(&context.context_index)
                    .ok_or(QueryError::BadCursor)?,
                self.text,
            )?;
            let locator_identity = crate::identity(context.plan.type_id, &ranked.staged.row)
                .map_err(|error| row_locator_invalid(&error))?;
            records.push((context.plan, record, locator_identity));
        }
        Ok(records)
    }
}

fn row_locator_invalid(message: &str) -> QueryError {
    QueryError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.to_owned(),
    )))
}

fn row_locator(plan: &Plan, mut record: Value) -> Result<RowLocator, QueryError> {
    let object = record
        .as_object_mut()
        .ok_or_else(|| row_locator_invalid("row record is not an object"))?;
    let Some(Value::Array(values)) = object.remove("values") else {
        return Err(row_locator_invalid("row record has no values array"));
    };
    if values.len() != plan.fields.len() {
        return Err(row_locator_invalid(
            "row field count does not match its rendered values",
        ));
    }
    let segment_id = object
        .remove("segment_id")
        .and_then(|value| value.as_str()?.parse::<i64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no segment_id"))?;
    let row_ordinal = object
        .remove("ordinal")
        .and_then(|value| value.as_str()?.parse::<u64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no ordinal"))?;
    let at = object
        .remove("timestamp")
        .and_then(|value| value.as_str()?.parse::<i64>().ok())
        .ok_or_else(|| row_locator_invalid("row record has no timestamp"))?;
    Ok(RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    })
}

/// Re-keys a rendered process row. A missing `pid` is invalid rendered data.
fn process_row_out(
    plan: &Plan,
    record: Value,
    identity: crate::RowIdentity,
) -> Result<ProcessRowOut, QueryError> {
    let RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    } = row_locator(plan, record)?;
    let mut fields = BTreeMap::new();
    let mut pid = None;
    let mut parent = None;
    for (field, value) in plan.fields.iter().zip(values) {
        match field.name.as_str() {
            "pid" => pid = value.as_i64(),
            "ppid" => parent = value.as_i64(),
            _ => {}
        }
        fields.insert(field.name.clone(), value);
    }
    let pid = pid.ok_or_else(|| row_locator_invalid("process row has no pid"))?;
    Ok(ProcessRowOut {
        pid,
        ppid: parent,
        segment_id,
        type_id: plan.type_id,
        row_ordinal,
        at,
        identity,
        fields,
    })
}

fn plain_row_out(
    plan: &Plan,
    record: Value,
    identity: crate::RowIdentity,
) -> Result<PlainRowOut, QueryError> {
    let RowLocator {
        segment_id,
        row_ordinal,
        at,
        values,
    } = row_locator(plan, record)?;
    let mut fields = BTreeMap::new();
    for (field, value) in plan.fields.iter().zip(values) {
        fields.insert(field.name.clone(), value);
    }
    if matches!(
        logical_section_name(plan.type_id),
        Some("pg_stat_statements" | "pg_store_plans")
    ) {
        fields.extend(derived_ratio_fields(&fields));
    }
    Ok(PlainRowOut {
        segment_id,
        type_id: plan.type_id,
        row_ordinal,
        at,
        identity,
        fields,
    })
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn ratio_field(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| fields.get(*name))
        .and_then(value_as_f64)
}

fn ratio_sum(fields: &BTreeMap<String, Value>, names: &[&str]) -> Option<f64> {
    let mut total = 0.0;
    for name in names {
        total += ratio_field(fields, &[name])?;
    }
    Some(total)
}

fn ratio_value(numerator: Option<f64>, denominator: Option<f64>) -> Value {
    match numerator.zip(denominator).map(|(n, d)| n / d) {
        Some(value) if value.is_finite() => json!(value),
        _ => Value::Null,
    }
}

fn derived_ratio_fields(fields: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let calls = ratio_field(fields, &["calls_per_second", "calls"]);
    let execution = ratio_field(fields, &["total_exec_time", "total_time"]);
    let hit = ratio_field(fields, &["shared_blks_hit"]);
    let read = ratio_field(fields, &["shared_blks_read"]);
    let planning = ratio_field(fields, &["total_plan_time"]);
    let blocks = ratio_sum(
        fields,
        &[
            "shared_blks_hit",
            "shared_blks_read",
            "local_blks_hit",
            "local_blks_read",
        ],
    );

    BTreeMap::from([
        (
            "derived_mean_exec_ms_per_call".to_owned(),
            ratio_value(execution, calls),
        ),
        (
            "derived_rows_per_call".to_owned(),
            ratio_value(ratio_field(fields, &["rows"]), calls),
        ),
        (
            "derived_blocks_per_call".to_owned(),
            ratio_value(blocks, calls),
        ),
        (
            "derived_hit_fraction".to_owned(),
            ratio_value(hit, hit.zip(read).map(|(hit, read)| hit + read)),
        ),
        (
            "derived_wal_per_call".to_owned(),
            ratio_value(ratio_field(fields, &["wal_bytes"]), calls),
        ),
        (
            "derived_plan_time_fraction".to_owned(),
            ratio_value(
                planning,
                planning
                    .zip(execution)
                    .map(|(planning, execution)| planning + execution),
            ),
        ),
        (
            "derived_cv".to_owned(),
            ratio_value(
                ratio_field(fields, &["stddev_exec_time", "stddev_time"]),
                ratio_field(fields, &["mean_exec_time", "mean_time"]),
            ),
        ),
    ])
}

#[cfg(test)]
use super::PAGE_SOURCE_VISITS;
