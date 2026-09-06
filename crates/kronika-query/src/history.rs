//! Projected full-resolution history, streamed per physical layout and identity.

use std::cell::RefCell;
use std::collections::BTreeSet;

use kronika_index::{
    DERIVED_HEALTH_TYPE_ID, INSTANCE_METADATA_TYPE_ID, INSTANCE_METADATA_V1_TYPE_ID,
    OS_PSI_TYPE_ID, visit_health_points,
};
use kronika_reader::{Cell, Row, Segment, SegmentKind};
use serde_json::{Value, json};

use super::projection::{
    Plan, apply_tail, plans, streaming_chunk_dictionary, validate_row_dictionary,
};
use super::render::{cell, projected_layout, record};
use super::selection::{active_tail, exact_segment as explicit_segment};
use crate::request::{ActiveCursor, DataRequest, Window};
use crate::{DatasetSegment, QueryDataset, QueryError, QuerySink, QueryStability};

const ROW_CHUNK_ROWS: usize = 512;

pub(crate) struct PreparedHistory {
    segment: Segment,
    logical_name: String,
    plans: Vec<Plan>,
    after: Option<ActiveCursor>,
    health: Option<HealthPlan>,
}

#[derive(Debug, Clone, Copy)]
struct HealthPlan {
    field_count: usize,
    psi_start_row: u64,
}

pub(crate) fn prepare(
    dataset: &dyn QueryDataset,
    request: DataRequest,
) -> Result<PreparedHistory, QueryError> {
    let segment_ref = explicit_segment(dataset, request.segment.segment_id)?;
    let tail = active_tail(dataset, &segment_ref, request.after)?;
    let segment = dataset.open(&segment_ref)?;
    let (plans, health) = if request.segment.section == "health" {
        (Vec::new(), Some(health_plan(&request, tail.as_ref())?))
    } else {
        let mut plans = plans(&segment, &request, true)?;
        apply_tail(&mut plans, tail.as_ref())?;
        (plans, None)
    };
    Ok(PreparedHistory {
        segment,
        logical_name: request.segment.section,
        plans,
        after: request.after,
        health,
    })
}

impl PreparedHistory {
    pub(crate) const fn stability(&self) -> QueryStability {
        match self.segment.kind() {
            SegmentKind::Finished => QueryStability::Immutable,
            SegmentKind::Active => QueryStability::Mutable,
        }
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "history",
                "segment": {
                    "id": self.segment.id().to_string(),
                    "kind": match self.segment.kind() {
                        SegmentKind::Finished => "finished",
                        SegmentKind::Active => "active",
                    },
                    "cursor": self.segment.active_position().map(|wal_position| json!({
                        "segment_id": self.segment.id().to_string(),
                        "wal_position": wal_position.to_string(),
                    })),
                },
                "logical_name": self.logical_name,
                "order": "physical_asc",
                "after": self.after.map(|cursor| json!({
                    "segment_id": cursor.segment_id.to_string(),
                    "wal_position": cursor.wal_position.to_string(),
                })),
            }))?)
        {
            return Ok(());
        }
        if let Some(health) = self.health {
            return self.stream_health(health, sink);
        }
        stream_plans(&self.segment, &self.logical_name, &self.plans, None, sink)
            .map(|_connected| ())
    }

    fn stream_health(&self, plan: HealthPlan, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "layout",
                "layout": health_layout(),
            }))?)
        {
            return Ok(());
        }
        let mut connected = true;
        let tail_timestamps = if self.after.is_some() {
            let mut timestamps = BTreeSet::new();
            self.segment.visit_rows(
                OS_PSI_TYPE_ID,
                &["ts"],
                plan.psi_start_row,
                usize::MAX,
                |_ordinal, row| {
                    if sink.cancelled() {
                        connected = false;
                        return false;
                    }
                    if let Some(Cell::Ts(timestamp)) = row.get("ts") {
                        timestamps.insert(*timestamp);
                    }
                    true
                },
            )?;
            Some(timestamps)
        } else {
            None
        };
        if !connected {
            return Ok(());
        }
        let mut ordinal = 0_u64;
        let mut failure = None;
        let sink = RefCell::new(sink);
        visit_health_points(
            &self.segment,
            || !sink.borrow().cancelled(),
            |point| {
                let point_ordinal = ordinal;
                ordinal = ordinal.saturating_add(1);
                if tail_timestamps
                    .as_ref()
                    .is_some_and(|timestamps| !timestamps.contains(&point.timestamp))
                {
                    return true;
                }
                let value = point.value.map_or(Value::Null, |value| json!(value));
                let values = vec![value; plan.field_count];
                match record(json!({
                    "record": "row",
                    "type_id": "0",
                    "ordinal": point_ordinal.to_string(),
                    "timestamp": point.timestamp.to_string(),
                    "identity": [],
                    "values": values,
                })) {
                    Ok(bytes) => connected = sink.borrow_mut().record(bytes),
                    Err(error) => {
                        failure = Some(error);
                        connected = false;
                    }
                }
                connected
            },
        )?;
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
}

fn health_plan(
    request: &DataRequest,
    tail: Option<&DatasetSegment>,
) -> Result<HealthPlan, QueryError> {
    for field in &request.fields {
        if field != "health" {
            return Err(QueryError::NoSuchColumn(field.clone()));
        }
    }
    if let Some(filter) = request.filters.first() {
        return Err(QueryError::BadFilter(filter.column.clone()));
    }
    let psi_start_row = tail
        .and_then(|segment| {
            segment
                .sections()
                .iter()
                .find(|section| section.type_id == OS_PSI_TYPE_ID)
        })
        .map_or(0, |section| section.rows);
    Ok(HealthPlan {
        field_count: request.fields.len().max(1),
        psi_start_row,
    })
}

fn emit_chunk(
    segment: &Segment,
    plan: &Plan,
    rows: &mut Vec<(u64, Row)>,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    if sink.cancelled() {
        return Ok(false);
    }
    let dictionary = streaming_chunk_dictionary(segment, rows)?;
    for (ordinal, row) in rows.drain(..) {
        if sink.cancelled() {
            return Ok(false);
        }
        validate_row_dictionary(&row, &dictionary)?;
        if !plan.matches(&row, &dictionary) {
            continue;
        }
        let identity = plan
            .contract
            .identity
            .iter()
            .map(|name| {
                row.get(name)
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let timestamp = plan
            .timestamp
            .and_then(|name| row.get(name))
            .map_or(Ok(Value::Null), |value| cell(value, &dictionary))?;
        let values = plan
            .fields
            .iter()
            .map(|field| {
                field
                    .column
                    .and_then(|name| row.get(name))
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !sink.record(record(json!({
            "record": "row",
            "type_id": plan.type_id.to_string(),
            "ordinal": ordinal.to_string(),
            "timestamp": timestamp,
            "identity": identity,
            "values": values,
        }))?) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn stream_plans(
    segment: &Segment,
    logical_name: &str,
    plans: &[Plan],
    window: Option<Window>,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    for plan in plans {
        if sink.cancelled() {
            return Ok(false);
        }
        let fields = plan
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.as_str(),
                    field.column.and_then(|name| plan.contract.column(name)),
                )
            })
            .collect::<Vec<_>>();
        if !sink.record(record(json!({
            "record": "layout",
            "layout": projected_layout(logical_name, plan.contract, &fields),
        }))?) {
            return Ok(false);
        }
        if !plan.applies() {
            continue;
        }
        let mut failure = None;
        let mut connected = true;
        let mut chunk = Vec::with_capacity(ROW_CHUNK_ROWS);
        segment.visit_rows(
            plan.type_id,
            &plan.projection,
            plan.start_row,
            usize::MAX,
            |ordinal, row| {
                if sink.cancelled() {
                    connected = false;
                    return false;
                }
                if window.is_some_and(|window| {
                    !plan
                        .timestamp
                        .and_then(|column| row.get(column))
                        .is_some_and(|cell| matches!(cell, Cell::Ts(ts) if window.contains(*ts)))
                }) {
                    return true;
                }
                chunk.push((ordinal, row));
                if chunk.len() < ROW_CHUNK_ROWS {
                    return true;
                }
                match emit_chunk(segment, plan, &mut chunk, sink) {
                    Ok(still_connected) => connected = still_connected,
                    Err(error) => failure = Some(error),
                }
                connected && failure.is_none()
            },
        )?;
        if failure.is_none() && connected && !chunk.is_empty() {
            match emit_chunk(segment, plan, &mut chunk, sink) {
                Ok(still_connected) => connected = still_connected,
                Err(error) => failure = Some(error),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if !connected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn health_layout() -> Value {
    json!({
        "logical_name": "health",
        "physical_name": "derived_os_health",
        "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
        "implementation": "kronika",
        "identity": [],
        "columns": [{
            "name": "os_health",
            "type": "u8",
            "class": "gauge",
            "unit": "percent",
            "nullable": true,
        }],
        "provenance": {
            "inputs": [
                INSTANCE_METADATA_TYPE_ID.to_string(),
                INSTANCE_METADATA_V1_TYPE_ID.to_string(),
                OS_PSI_TYPE_ID.to_string(),
                "1001001",
                "1001002",
                "1001004",
            ],
        },
    })
}
