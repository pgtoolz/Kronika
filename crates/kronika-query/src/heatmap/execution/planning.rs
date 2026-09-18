//! Heatmap validation, section sharing, and physical scan plans.

use std::collections::{HashMap, HashSet};

use kronika_reader::Segment;
use kronika_registry::{ColumnClass, Unit, contract, logical_section_name, registry};

use super::{Binding, HeatmapError, ItemSpec, PhysicalPlan, SharedSectionSpec};

use crate::heatmap::query::{HeatmapItemQuery, HeatmapView, MAX_FIELDS, MAX_TOP};
use crate::{STATEMENTS_SECTION, row_key};

pub(super) fn normalize_items(
    items: &[HeatmapItemQuery],
) -> Result<(Vec<ItemSpec>, Vec<usize>), HeatmapError> {
    let mut unique = Vec::new();
    let mut positions = HashMap::new();
    let mut original_to_unique = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(position) = positions.get(item).copied() {
            original_to_unique.push(position);
            continue;
        }
        let (class, unit, labels) = validate_item(item, index)?;
        let position = unique.len();
        positions.insert(item.clone(), position);
        original_to_unique.push(position);
        unique.push(ItemSpec {
            query: item.clone(),
            class,
            unit,
            labels,
            first_index: index,
        });
    }
    Ok((unique, original_to_unique))
}

pub(super) fn shared_section_specs(specs: &[ItemSpec]) -> (Vec<SharedSectionSpec>, Vec<usize>) {
    let mut sections = Vec::<SharedSectionSpec>::new();
    let mut positions = HashMap::<&str, usize>::new();
    let mut accumulator_sections = Vec::with_capacity(specs.len());
    for spec in specs {
        let section = spec.query.ranking.section.as_str();
        let position = positions.get(section).copied().unwrap_or_else(|| {
            let position = sections.len();
            positions.insert(section, position);
            sections.push(SharedSectionSpec {
                name: section.to_owned(),
                labels: Vec::new(),
                first_index: spec.first_index,
            });
            position
        });
        if spec.query.view.groups().is_empty() {
            for label in &spec.labels {
                if !sections[position].labels.contains(label) {
                    sections[position].labels.push(label.clone());
                }
            }
        }
        accumulator_sections.push(position);
    }
    (sections, accumulator_sections)
}

#[expect(
    clippy::too_many_lines,
    reason = "validation keeps every indexed ranking error in request order"
)]
fn validate_item(
    item: &HeatmapItemQuery,
    index: usize,
) -> Result<(ColumnClass, Option<Unit>, Vec<String>), HeatmapError> {
    let ranking = &item.ranking;
    if ranking.section.is_empty() || ranking.section.len() > 128 {
        return Err(HeatmapError::invalid_parameter(
            index,
            "section must contain 1 to 128 UTF-8 bytes",
            "section",
        ));
    }
    match item.view {
        HeatmapView::RankingOnly if ranking.fields.len() != 1 => {
            return Err(HeatmapError::invalid_parameter(
                index,
                "an Overview result must contain exactly one field",
                "field",
            ));
        }
        HeatmapView::Grid { .. } if !(1..=MAX_FIELDS).contains(&ranking.fields.len()) => {
            return Err(HeatmapError::invalid_parameter(
                index,
                format!("fields must contain 1 to {MAX_FIELDS} names"),
                "field",
            ));
        }
        HeatmapView::RankingOnly | HeatmapView::Grid { .. } => {}
    }
    let mut seen = HashSet::new();
    for field in &ranking.fields {
        if field.is_empty() || !seen.insert(field) {
            return Err(HeatmapError::invalid_parameter(
                index,
                format!("field {field:?} is empty or repeated"),
                "field",
            ));
        }
    }
    if !(1..=MAX_TOP).contains(&ranking.top) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("top must be between 1 and {MAX_TOP}, got {}", ranking.top),
            "top",
        ));
    }
    if !item.scope.allows_rows(&ranking.section) {
        return Err(HeatmapError::invalid_parameter(
            index,
            format!("scope=workload applies only to {STATEMENTS_SECTION}"),
            "scope",
        ));
    }

    let wanted_type = item.view.type_id();
    let contracts: Vec<_> = registry()
        .iter()
        .filter(|contract| {
            logical_section_name(contract.type_id.get()) == Some(ranking.section.as_str())
                && wanted_type.is_none_or(|wanted| wanted == contract.type_id.get())
        })
        .collect();
    if contracts.is_empty() {
        let mut seen = HashSet::new();
        let valid_options = registry()
            .iter()
            .filter_map(|contract| logical_section_name(contract.type_id.get()))
            .filter(|section| seen.insert(*section))
            .map(str::to_owned)
            .collect();
        return Err(HeatmapError::no_such_section(
            index,
            "no such logical section",
            valid_options,
        ));
    }
    let numeric_options = || {
        let mut seen = HashSet::new();
        contracts
            .iter()
            .flat_map(|contract| contract.columns)
            .filter(|column| {
                matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge)
                    && seen.insert(column.name)
            })
            .map(|column| column.name.to_owned())
            .collect::<Vec<_>>()
    };
    let mut class = None;
    let mut unit = None;
    for field in &ranking.fields {
        let columns: Vec<_> = contracts
            .iter()
            .filter_map(|contract| contract.column(field))
            .collect();
        if columns.is_empty() {
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {field:?}"),
                field.clone(),
                numeric_options(),
            ));
        }
        for column in columns {
            if !matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge) {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!("column {field:?} is not numeric"),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if class
                .replace(column.class)
                .is_some_and(|seen| seen != column.class)
            {
                return Err(HeatmapError::no_such_column(
                    index,
                    format!(
                        "fields carry different classes: {}",
                        ranking.fields.join("+")
                    ),
                    field.clone(),
                    numeric_options(),
                ));
            }
            if unit
                .replace(column.unit)
                .is_some_and(|seen| seen != column.unit)
            {
                let fields = ranking.fields.join("+");
                return Err(HeatmapError::mixed_units(
                    index,
                    format!("fields carry different units: {fields}"),
                    fields,
                ));
            }
        }
    }
    for group in item.view.groups() {
        if group.is_empty()
            || !contracts
                .iter()
                .any(|contract| contract.column(group).is_some())
        {
            let mut seen = HashSet::new();
            let valid_options = contracts
                .iter()
                .flat_map(|contract| contract.columns)
                .filter(|column| seen.insert(column.name))
                .map(|column| column.name.to_owned())
                .collect();
            return Err(HeatmapError::no_such_column(
                index,
                format!("no such column {group:?}"),
                group.clone(),
                valid_options,
            ));
        }
    }
    let mut labels = Vec::new();
    let mut label_seen = HashSet::new();
    for contract in contracts {
        for column in contract.columns {
            if column.class == ColumnClass::Label
                && !row_key::is_detail_text(&ranking.section, column.name)
                && label_seen.insert(column.name)
            {
                labels.push(column.name.to_owned());
            }
        }
    }
    let class = class.ok_or_else(|| {
        HeatmapError::invalid(index, "numeric field validation found no quantity class")
    })?;
    Ok((class, unit.unwrap_or(None), labels))
}

pub(super) fn physical_plans(
    segment: &Segment,
    specs: &[ItemSpec],
    sections: &[SharedSectionSpec],
    accumulator_sections: &[usize],
) -> Vec<PhysicalPlan> {
    let mut plans = Vec::new();
    for (section, shared) in sections.iter().enumerate() {
        for (type_id, stored) in segment.layouts(&shared.name) {
            let Some(contract) = contract(type_id) else {
                continue;
            };
            let Some(timestamp) = contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name)
            else {
                continue;
            };
            let mut projection = vec![timestamp];
            projection.extend(row_key::identity_columns(contract));
            let labels = shared
                .labels
                .iter()
                .map(|name| contract.column(name).map(|column| column.name))
                .collect::<Vec<_>>();
            projection.extend(labels.iter().flatten().copied());
            let mut bindings = Vec::new();
            let mut first_index = usize::MAX;
            for (accumulator, spec) in specs.iter().enumerate() {
                if accumulator_sections[accumulator] != section
                    || spec
                        .query
                        .view
                        .type_id()
                        .is_some_and(|wanted| wanted != type_id)
                {
                    continue;
                }
                let metrics: Vec<_> = spec
                    .query
                    .ranking
                    .fields
                    .iter()
                    .filter_map(|name| contract.column(name).map(|column| column.name))
                    .collect();
                if metrics.is_empty() {
                    continue;
                }
                let groups = spec
                    .query
                    .view
                    .groups()
                    .iter()
                    .map(|name| contract.column(name).map(|column| column.name))
                    .collect::<Vec<_>>();
                projection.extend(metrics.iter().copied());
                projection.extend(groups.iter().flatten().copied());
                let scope_column = spec
                    .query
                    .scope
                    .filters(&shared.name)
                    .then(|| contract.column("query").map(|column| column.name))
                    .flatten();
                projection.extend(scope_column);
                bindings.push(Binding {
                    accumulator,
                    metrics,
                    groups,
                    workload: scope_column.is_some(),
                });
                first_index = first_index.min(spec.first_index);
            }
            if bindings.is_empty() {
                continue;
            }
            projection.sort_unstable();
            projection.dedup();
            plans.push(PhysicalPlan {
                section,
                type_id,
                contract,
                rows: stored.rows,
                timestamp,
                projection,
                labels,
                bindings,
                first_index,
            });
        }
    }
    plans
}
