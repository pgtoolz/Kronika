//! Snapshot search evaluation over stored and derived values.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment};
use kronika_registry::{contract, logical_section_name};

use super::{IdentityCell, OrderedNumber, PageContext, ProcessUsers, ordered_cell};

use crate::QueryError;
use crate::exact_product::compare_products;
use crate::projection::{Plan, resolved_dictionary};
use crate::snapshot::paging::{counter_sum, value_sum};
use crate::snapshot::search::{
    Quantity, SearchClause, SearchOperator, SearchValue, StructuredSearch, result_field,
    search_fields, search_value_matches,
};
use crate::statement_scope::plan_statement_query_id_columns;

pub(super) fn search_columns(
    logical_name: &str,
    plan: &Plan,
    search: &StructuredSearch,
) -> Vec<&'static str> {
    let mut columns = Vec::new();
    for clause in &search.clauses {
        let mut wanted = search_clause_columns(logical_name, plan, clause.key);
        if let Some((_clause, field)) = search
            .result_clauses(logical_name)
            .find(|(candidate, _field)| candidate.key == clause.key)
        {
            wanted.extend(
                field
                    .dependencies
                    .iter()
                    .filter_map(|name| plan.contract.column(name).map(|column| column.name)),
            );
        }
        for column in wanted {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
    }
    columns
}

pub(super) fn clock_ticks_per_second(segment: &Segment) -> Result<Option<u128>, QueryError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("instance_metadata") {
            continue;
        }
        let Some(column) =
            contract(type_id).and_then(|layout| layout.column("clock_ticks_per_sec"))
        else {
            continue;
        };
        let mut stored = None;
        segment.visit_rows(type_id, &[column.name], 0, 1, |_ordinal, row| {
            stored = row
                .get(column.name)
                .and_then(ordered_cell)
                .and_then(|value| {
                    let OrderedNumber::Integer(value) = value else {
                        return None;
                    };
                    u128::try_from(value).ok().filter(|value| *value > 0)
                });
            false
        })?;
        if stored.is_some() {
            return Ok(stored);
        }
    }
    Ok(None)
}

pub(super) fn postgres_block_size(segment: &Segment) -> Result<Option<u128>, QueryError> {
    for (type_id, _rows) in segment.sections() {
        if logical_section_name(type_id) != Some("pg_settings") {
            continue;
        }
        let Some(layout) = contract(type_id) else {
            continue;
        };
        let (Some(name), Some(setting)) = (layout.column("name"), layout.column("setting")) else {
            continue;
        };
        let mut candidates = Vec::new();
        let mut ids = HashSet::new();
        segment.visit_rows(
            type_id,
            &[name.name, setting.name],
            0,
            usize::MAX,
            |_ordinal, row| {
                let (Some(Cell::StrId(name_id)), Some(Cell::StrId(setting_id))) =
                    (row.get(name.name), row.get(setting.name))
                else {
                    return true;
                };
                ids.insert(*name_id);
                ids.insert(*setting_id);
                candidates.push((*name_id, *setting_id));
                true
            },
        )?;
        let dictionary = resolved_dictionary(segment, &ids)?;
        for (name_id, setting_id) in candidates {
            let Some(Resolved::Str(name)) = dictionary.resolve(name_id) else {
                continue;
            };
            if name != b"block_size" {
                continue;
            }
            let Some(Resolved::Str(setting)) = dictionary.resolve(setting_id) else {
                continue;
            };
            let Ok(setting) = std::str::from_utf8(setting) else {
                continue;
            };
            if let Ok(value) = setting.parse::<u128>()
                && value > 0
            {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

#[expect(
    variant_size_differences,
    reason = "exact quantity factors stay inline so per-row comparisons do not allocate"
)]
enum SearchMetricValue {
    Exact {
        numerator: ProductFactors,
        denominator: ProductFactors,
    },
    Float(f64),
}

#[derive(Clone, Copy)]
struct ProductFactors {
    values: [u128; 4],
    len: usize,
}

impl ProductFactors {
    const fn one(value: u128) -> Self {
        Self {
            values: [value, 1, 1, 1],
            len: 1,
        }
    }

    const fn two(first: u128, second: u128) -> Self {
        Self {
            values: [first, second, 1, 1],
            len: 2,
        }
    }

    fn push(mut self, value: u128) -> Option<Self> {
        let slot = self.values.get_mut(self.len)?;
        *slot = value;
        self.len += 1;
        Some(self)
    }

    fn as_slice(&self) -> &[u128] {
        &self.values[..self.len]
    }
}

impl SearchMetricValue {
    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics must be scaled in their native f64 domain"
    )]
    fn scale(self, numerator_scale: u128, denominator_scale: u128) -> Option<Self> {
        match self {
            Self::Exact {
                numerator,
                denominator,
            } => Some(Self::Exact {
                numerator: numerator.push(numerator_scale)?,
                denominator: denominator.push(denominator_scale)?,
            }),
            Self::Float(value) => {
                let scaled = value * numerator_scale as f64 / denominator_scale as f64;
                scaled.is_finite().then_some(Self::Float(scaled))
            }
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "stored floating metrics are compared to the closest f64 threshold"
    )]
    fn matches(&self, operator: SearchOperator, quantity: &Quantity) -> bool {
        let ordering = match self {
            Self::Exact {
                numerator,
                denominator,
            } => {
                let (Some(left), Some(right)) = (
                    numerator.push(quantity.denominator),
                    denominator.push(quantity.numerator),
                ) else {
                    return false;
                };
                compare_products(left.as_slice(), right.as_slice())
            }
            Self::Float(value) => {
                let threshold = quantity.numerator as f64 / quantity.denominator as f64;
                if !value.is_finite() || !threshold.is_finite() {
                    return false;
                }
                value.total_cmp(&threshold)
            }
        };
        match operator {
            SearchOperator::Greater => ordering == Ordering::Greater,
            SearchOperator::Less => ordering == Ordering::Less,
            SearchOperator::Colon => false,
        }
    }
}

pub(super) fn result_search_matches(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    clause: &SearchClause,
) -> bool {
    let SearchValue::Quantity(quantity) = &clause.value else {
        return false;
    };
    let Some(field) = result_field(context.logical_name, clause.key) else {
        return false;
    };
    search_metric(context, row, identity, field.metric)
        .is_some_and(|value| value.matches(clause.operator, quantity))
}

fn search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match context.logical_name {
        "os_process" => process_search_metric(context, row, identity, metric),
        "pg_stat_statements" | "pg_store_plans" => {
            postgres_search_metric(context, row, identity, metric)
        }
        "pg_stat_activity" => activity_search_metric(row, metric),
        "pg_stat_progress_vacuum" => vacuum_search_metric(row, metric),
        "pg_stat_database" => database_search_metric(context, row, identity, metric),
        _ => None,
    }
}

/// Compares recorded xid/xmin-age gauges directly; no predecessor or rate is
/// used.
fn activity_search_metric(row: &Row, metric: &str) -> Option<SearchMetricValue> {
    match metric {
        "backend_xid_age" => gauge_metric(row, "backend_xid_age", 1, 1),
        "backend_xmin_age" => gauge_metric(row, "backend_xmin_age", 1, 1),
        _ => None,
    }
}

/// Compares recorded heap-block gauges directly; no predecessor or rate is
/// used.
fn vacuum_search_metric(row: &Row, metric: &str) -> Option<SearchMetricValue> {
    match metric {
        "heap_blks_total" => gauge_metric(row, "heap_blks_total", 1, 1),
        "heap_blks_scanned" => gauge_metric(row, "heap_blks_scanned", 1, 1),
        "heap_blks_vacuumed" => gauge_metric(row, "heap_blks_vacuumed", 1, 1),
        _ => None,
    }
}

/// Compares `numbackends` as a recorded gauge and the four cumulative fields
/// as interval rates.
fn database_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match metric {
        "numbackends" => gauge_metric(row, "numbackends", 1, 1),
        "xact_commit" => rate_metric(context, row, identity, &["xact_commit"], 1, 1),
        "xact_rollback" => rate_metric(context, row, identity, &["xact_rollback"], 1, 1),
        "deadlocks" => rate_metric(context, row, identity, &["deadlocks"], 1, 1),
        "temp_bytes" => rate_metric(context, row, identity, &["temp_bytes"], 1, 1),
        _ => None,
    }
}

fn process_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    match metric {
        "rss" => gauge_metric(row, "rmem_kb", 1_024, 1),
        "vsz" => gauge_metric(row, "vmem_kb", 1_024, 1),
        "swap" => gauge_metric(row, "vswap_kb", 1_024, 1),
        "threads" => gauge_metric(row, "num_threads", 1, 1),
        "cpu_cores" | "user_cpu_cores" | "system_cpu_cores" => {
            let columns: &[&str] = match metric {
                "user_cpu_cores" => &["utime"],
                "system_cpu_cores" => &["stime"],
                _ => &["utime", "stime"],
            };
            let ticks = context.clock_ticks_per_second?;
            rate_metric(context, row, identity, columns, 1_000_000, ticks)
        }
        "disk_read_rate" => rate_metric(context, row, identity, &["read_bytes"], 1_000_000, 1),
        "disk_write_rate" => rate_metric(context, row, identity, &["write_bytes"], 1_000_000, 1),
        "logical_read_rate" => rate_metric(context, row, identity, &["rchar"], 1_000_000, 1),
        "logical_write_rate" => rate_metric(context, row, identity, &["wchar"], 1_000_000, 1),
        "read_syscall_rate" => rate_metric(context, row, identity, &["syscr"], 1_000_000, 1),
        "write_syscall_rate" => rate_metric(context, row, identity, &["syscw"], 1_000_000, 1),
        "major_fault_rate" => rate_metric(context, row, identity, &["majflt"], 1_000_000, 1),
        "minor_fault_rate" => rate_metric(context, row, identity, &["minflt"], 1_000_000, 1),
        "context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw", "nivcsw"], 1_000_000, 1)
        }
        "voluntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nvcsw"], 1_000_000, 1)
        }
        "involuntary_context_switch_rate" => {
            rate_metric(context, row, identity, &["nivcsw"], 1_000_000, 1)
        }
        "run_delay" => rate_metric(context, row, identity, &["rundelay_ns"], 1, 1),
        "block_io_delay" => rate_metric(
            context,
            row,
            identity,
            &["blkdelay_ticks"],
            1_000_000_000,
            context.clock_ticks_per_second?,
        ),
        _ => None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixed public PostgreSQL metric adapter stays exhaustive and fork-transparent"
)]
fn postgres_search_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    metric: &str,
) -> Option<SearchMetricValue> {
    let execution = preferred_column(context.plan, "total_exec_time", "total_time");
    let mean = preferred_column(context.plan, "mean_exec_time", "mean_time");
    let stddev = preferred_column(context.plan, "stddev_exec_time", "stddev_time");
    if metric == "calls" {
        return gauge_metric(row, "calls", 1, 1);
    }
    let direct_rate = match metric {
        "call_rate" => Some("calls"),
        "exec_time_rate" => execution,
        "row_rate" => Some("rows"),
        "plan_rate" => Some("plans"),
        "planning_time_rate" => Some("total_plan_time"),
        "slow_call_rate" => Some("slow_log_calls"),
        "shared_read_time_rate" => {
            preferred_column(context.plan, "shared_blk_read_time", "blk_read_time")
        }
        "shared_write_time_rate" => {
            preferred_column(context.plan, "shared_blk_write_time", "blk_write_time")
        }
        "local_read_time_rate" => Some("local_blk_read_time"),
        "local_write_time_rate" => Some("local_blk_write_time"),
        "temp_read_time_rate" => Some("temp_blk_read_time"),
        "temp_write_time_rate" => Some("temp_blk_write_time"),
        "wal_rate" => Some("wal_bytes"),
        _ => None,
    };
    if let Some(column) = direct_rate {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1);
    }
    let buffer_column = match metric {
        "shared_buffer_hit_rate" => Some("shared_blks_hit"),
        "shared_buffer_read_rate" => Some("shared_blks_read"),
        "shared_buffer_dirty_rate" => Some("shared_blks_dirtied"),
        "shared_buffer_write_rate" => Some("shared_blks_written"),
        "local_buffer_hit_rate" => Some("local_blks_hit"),
        "local_buffer_read_rate" => Some("local_blks_read"),
        "local_buffer_dirty_rate" => Some("local_blks_dirtied"),
        "local_buffer_write_rate" => Some("local_blks_written"),
        "temp_buffer_read_rate" => Some("temp_blks_read"),
        "temp_buffer_write_rate" => Some("temp_blks_written"),
        _ => None,
    };
    if let Some(column) = buffer_column {
        return rate_metric(context, row, identity, &[column], 1_000_000, 1)
            .and_then(|value| value.scale(context.block_size?, 1));
    }
    match metric {
        "mean_exec" => counter_ratio_metric(context, row, identity, &[execution?], &["calls"], 1),
        "rows_per_call" => counter_ratio_metric(context, row, identity, &["rows"], &["calls"], 1),
        "wal_per_call" => {
            counter_ratio_metric(context, row, identity, &["wal_bytes"], &["calls"], 1)
        }
        "planning_share" => counter_ratio_metric(
            context,
            row,
            identity,
            &["total_plan_time"],
            &["total_plan_time", execution?],
            100,
        ),
        "buffer_hit" => counter_ratio_metric(
            context,
            row,
            identity,
            &["shared_blks_hit"],
            &["shared_blks_hit", "shared_blks_read"],
            100,
        ),
        "buffer_per_call" => counter_ratio_metric(
            context,
            row,
            identity,
            &[
                "shared_blks_hit",
                "shared_blks_read",
                "shared_blks_dirtied",
                "shared_blks_written",
                "local_blks_hit",
                "local_blks_read",
                "local_blks_dirtied",
                "local_blks_written",
                "temp_blks_read",
                "temp_blks_written",
            ],
            &["calls"],
            context.block_size?,
        ),
        "exec_cv" => value_ratio_metric(row, &[stddev?], &[mean?], 1),
        "min_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "min_exec_time", "min_time")?,
            1,
            1,
        ),
        "max_exec_since_reset" => gauge_metric(
            row,
            preferred_column(context.plan, "max_exec_time", "max_time")?,
            1,
            1,
        ),
        "mean_exec_since_reset" => gauge_metric(row, mean?, 1, 1),
        "stddev_exec_since_reset" => gauge_metric(row, stddev?, 1, 1),
        _ => None,
    }
}

fn preferred_column(
    plan: &Plan,
    preferred: &'static str,
    fallback: &'static str,
) -> Option<&'static str> {
    plan.contract
        .column(preferred)
        .map(|column| column.name)
        .or_else(|| plan.contract.column(fallback).map(|column| column.name))
}

fn gauge_metric(
    row: &Row,
    column: &'static str,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    ordered_metric(
        ordered_cell(row.get(column)?)?,
        numerator_scale,
        denominator_scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn rate_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    columns: &[&'static str],
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    let elapsed = u128::try_from(context.elapsed_for(row)?)
        .ok()
        .filter(|value| *value > 0)?;
    let before = context.predecessor(row, identity)?;
    let delta = counter_sum(row, before, columns, false)?;
    match delta {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::two(elapsed, denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted =
                value * numerator_scale as f64 / elapsed as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

fn counter_ratio_metric(
    context: &PageContext<'_>,
    row: &Row,
    identity: &[IdentityCell],
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    let _elapsed = context.elapsed_for(row)?;
    let before = context.predecessor(row, identity)?;
    let numerator = counter_sum(row, before, numerator_columns, false)?;
    let denominator = counter_sum(row, before, denominator_columns, false)?;
    ratio_metric(numerator, denominator, scale)
}

fn value_ratio_metric(
    row: &Row,
    numerator_columns: &[&'static str],
    denominator_columns: &[&'static str],
    scale: u128,
) -> Option<SearchMetricValue> {
    ratio_metric(
        value_sum(row, numerator_columns)?,
        value_sum(row, denominator_columns)?,
        scale,
    )
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating counters preserve their recorded f64 arithmetic"
)]
fn ratio_metric(
    numerator: OrderedNumber,
    denominator: OrderedNumber,
    scale: u128,
) -> Option<SearchMetricValue> {
    match (numerator, denominator) {
        (OrderedNumber::Integer(numerator), OrderedNumber::Integer(denominator))
            if numerator >= 0 && denominator > 0 =>
        {
            Some(SearchMetricValue::Exact {
                numerator: ProductFactors::two(u128::try_from(numerator).ok()?, scale),
                denominator: ProductFactors::one(u128::try_from(denominator).ok()?),
            })
        }
        (numerator, denominator) => {
            let denominator = denominator.as_f64();
            let ratio = numerator.as_f64() / denominator * scale as f64;
            (denominator > 0.0 && ratio.is_finite()).then_some(SearchMetricValue::Float(ratio))
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "stored floating gauges preserve their recorded f64 value"
)]
fn ordered_metric(
    value: OrderedNumber,
    numerator_scale: u128,
    denominator_scale: u128,
) -> Option<SearchMetricValue> {
    match value {
        OrderedNumber::Integer(value) if value >= 0 => Some(SearchMetricValue::Exact {
            numerator: ProductFactors::two(u128::try_from(value).ok()?, numerator_scale),
            denominator: ProductFactors::one(denominator_scale),
        }),
        OrderedNumber::Float(value) => {
            let converted = value * numerator_scale as f64 / denominator_scale as f64;
            converted
                .is_finite()
                .then_some(SearchMetricValue::Float(converted))
        }
        OrderedNumber::Integer(_) => None,
    }
}

pub(super) fn search_clause_columns(
    logical_name: &str,
    plan: &Plan,
    key: &str,
) -> Vec<&'static str> {
    let Some(field) = search_fields(logical_name)
        .iter()
        .find(|field| field.key == key)
    else {
        return Vec::new();
    };
    let wanted: &[&str] = if logical_name == "pg_store_plans" && key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        field.columns
    };
    wanted
        .iter()
        .filter_map(|name| plan.contract.column(name).map(|column| column.name))
        .collect()
}

pub(super) fn search_clause_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    clause: &SearchClause,
) -> bool {
    let excludes_zero_query =
        logical_name == "pg_store_plans" && plan.type_id == 1_004_001 && clause.key == "query_id";
    if logical_name == "os_process" {
        let name_matches = |uid_column| {
            process_users
                .and_then(|users| users.for_row(row, uid_column))
                .is_some_and(|name| search_value_matches(name, &clause.value))
        };
        match clause.key {
            "user" => return name_matches("uid"),
            "effective_user" => return name_matches("euid"),
            "text" => {
                if name_matches("uid") || name_matches("euid") {
                    return true;
                }
                return ["comm", "cmdline"].iter().any(|column| {
                    row.get(column)
                        .and_then(|value| searchable_text(value, dictionary))
                        .is_some_and(|text| search_value_matches(&text, &clause.value))
                });
            }
            _ => {}
        }
    }
    let columns = if logical_name == "pg_store_plans" && clause.key == "query_id" {
        plan_statement_query_id_columns(plan.type_id)
    } else {
        clause.columns
    };
    columns.iter().any(|column| {
        row.get(column)
            .and_then(|value| searchable_text(value, dictionary))
            .is_some_and(|text| {
                (!excludes_zero_query || text != "0") && search_value_matches(&text, &clause.value)
            })
    })
}

pub(super) fn search_matches(
    logical_name: &str,
    plan: &Plan,
    row: &Row,
    dictionary: &Dictionary,
    process_users: Option<&ProcessUsers>,
    search: &StructuredSearch,
) -> bool {
    search.matches_member(|clause| {
        search_clause_matches(logical_name, plan, row, dictionary, process_users, clause)
    })
}

fn searchable_text<'a>(value: &Cell, dictionary: &'a Dictionary) -> Option<Cow<'a, str>> {
    match value {
        Cell::I16(value) => Some(Cow::Owned(value.to_string())),
        Cell::I32(value) => Some(Cow::Owned(value.to_string())),
        Cell::I64(value) | Cell::Ts(value) => Some(Cow::Owned(value.to_string())),
        Cell::U32(value) => Some(Cow::Owned(value.to_string())),
        Cell::U64(value) => Some(Cow::Owned(value.to_string())),
        Cell::F64(value) if value.is_finite() => Some(Cow::Owned(value.to_string())),
        Cell::Bool(value) => Some(Cow::Owned(value.to_string())),
        Cell::StrId(id) => dictionary
            .resolve(*id)
            .and_then(|resolved| std::str::from_utf8(resolved.stored_bytes()).ok())
            .map(Cow::Borrowed),
        Cell::Null | Cell::ListI32(_) | Cell::F64(_) => None,
    }
}
