//! NDJSON emission for completed heatmap grids.

use std::collections::BTreeMap;

use kronika_registry::contract;
use serde_json::{Value, json};

use crate::heatmap::execution::render::public_identity_name;
use crate::heatmap::result::HeatmapItemResult;
use crate::render::record;
use crate::{QueryError, QuerySink};

pub(in crate::heatmap) fn stream_grid(
    item: &HeatmapItemResult,
    from: i64,
    to: i64,
    sink: &mut dyn QuerySink,
) -> Result<(), QueryError> {
    let grid = item.grid.as_ref().ok_or_else(|| {
        QueryError::BadFilter("streamed heatmap requires a grid result".to_owned())
    })?;
    let grouped = !grid.group_names.is_empty();
    let mut header = json!({
        "record": "heatmap",
        "from": from.to_string(),
        "to": to.to_string(),
        "section": item.ranking.section,
        "fields": item.ranking.fields,
        "class": item.class.code(),
        "summary": item.summary,
        "labels": grid.label_names,
        "top": if grouped { grid.groups.len() } else { item.entities.len() },
        "entity_count": item.entity_count,
        "others_count": item.entity_count.saturating_sub(
            u64::try_from(if grouped { grid.groups.len() } else { item.entities.len() })
                .unwrap_or(u64::MAX)
        ),
        "out_of_order": item.out_of_order.to_string(),
        "intervals": grid.intervals,
    });
    if grouped {
        let Some(header) = header.as_object_mut() else {
            return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "heatmap header did not serialize as an object",
            ))));
        };
        header.insert("group".to_owned(), json!(grid.group_names));
    }
    if sink.cancelled() || !sink.record(record(header)?) {
        return Ok(());
    }
    if grouped {
        for group in &grid.groups {
            if sink.cancelled()
                || !sink.record(record(json!({
                    "record": "heatmap_row",
                    "type_id": "0",
                    "identity": group.values,
                    "labels": [],
                    "members": group.members,
                    "total": group.total,
                    "cells": group.cells,
                }))?)
            {
                return Ok(());
            }
        }
    } else {
        for entity in &item.entities {
            let type_id = entity.detail_locator.type_id;
            let labels: Vec<Value> = grid
                .label_names
                .iter()
                .map(|name| entity.labels.get(name).cloned().unwrap_or(Value::Null))
                .collect();
            if sink.cancelled()
                || !sink.record(record(json!({
                    "record": "heatmap_row",
                    "type_id": type_id.to_string(),
                    "identity": stream_identity(type_id, &entity.identity),
                    "labels": labels,
                    "total": entity.total,
                    "cells": entity.cells,
                }))?)
            {
                return Ok(());
            }
        }
    }
    if !sink.record(record(json!({
        "record": "heatmap_band",
        "band": "totals",
        "total": grid.totals.total,
        "cells": grid.totals.cells,
    }))?) {
        return Ok(());
    }
    if !sink.record(record(json!({
        "record": "heatmap_band",
        "band": "others",
        "total": grid.others.total,
        "cells": grid.others.cells,
    }))?) {
        return Ok(());
    }
    Ok(())
}

fn stream_identity(type_id: u32, identity: &BTreeMap<String, Value>) -> Vec<Value> {
    let Some(contract) = contract(type_id) else {
        return identity.values().cloned().collect();
    };
    contract
        .identity
        .iter()
        .map(|name| {
            identity
                .get(public_identity_name(name))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect()
}
