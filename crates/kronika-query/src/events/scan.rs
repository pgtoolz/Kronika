//! Bounded event scans over captured segments.

use kronika_reader::{Cell, Row, Segment};
use serde_json::{Map, Value};

use super::{EventDataRow, EventSource, ROW_CHUNK_ROWS};

use crate::projection::{Plan, plans, streaming_chunk_dictionary, validate_row_dictionary};
use crate::render::cell;
use crate::request::{DataRequest, Filter, SegmentRequest};
use crate::time::TimeRange;
use crate::{DatasetSegment, QueryError, QuerySink, row_key};

pub(super) fn carries_selected(
    segment: &DatasetSegment,
    sources: &[EventSource],
    settings: bool,
) -> bool {
    segment.sections().iter().any(|section| {
        let name = kronika_registry::logical_section_name(section.type_id);
        (settings && name == Some("pg_settings"))
            || sources.iter().any(|source| name == Some(source.as_str()))
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one low-level section scan receives its exact storage coordinates and sinks"
)]
pub(super) fn collect_section(
    segment: &Segment,
    segment_id: i64,
    logical_name: &str,
    fields: &[&str],
    filters: &[Filter],
    range: TimeRange,
    output: &mut impl FnMut(EventDataRow) -> Result<(), QueryError>,
    sink: &dyn QuerySink,
) -> Result<(), QueryError> {
    let request = DataRequest {
        segment: SegmentRequest {
            segment_id,
            section: logical_name.to_owned(),
        },
        fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        filters: filters.to_vec(),
        type_id: None,
        after: None,
    };
    let section_plans = match plans(segment, &request, true) {
        Ok(section_plans) => section_plans,
        Err(QueryError::NoSuchSection) => return Ok(()),
        Err(error) => return Err(error),
    };
    for plan in &section_plans {
        collect_plan(segment, segment_id, plan, range, output, sink)?;
    }
    Ok(())
}

fn collect_plan(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    range: TimeRange,
    output: &mut impl FnMut(EventDataRow) -> Result<(), QueryError>,
    sink: &dyn QuerySink,
) -> Result<(), QueryError> {
    if !plan.applies() {
        return Ok(());
    }
    let Some(timestamp_column) = plan.timestamp else {
        return Ok(());
    };
    let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(ROW_CHUNK_ROWS);
    let mut failure = None;
    let mut was_cancelled = false;
    segment.visit_rows(
        plan.type_id,
        &plan.projection,
        plan.start_row,
        usize::MAX,
        |ordinal, row| {
            if sink.cancelled() {
                was_cancelled = true;
                return false;
            }
            if !row
                .get(timestamp_column)
                .is_some_and(|cell| matches!(cell, Cell::Ts(at) if range.contains(*at)))
            {
                return true;
            }
            chunk.push((ordinal, row));
            if chunk.len() < ROW_CHUNK_ROWS {
                return true;
            }
            if let Err(error) = append_chunk(
                segment,
                segment_id,
                plan,
                timestamp_column,
                &mut chunk,
                output,
            ) {
                failure = Some(error);
                return false;
            }
            true
        },
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    if was_cancelled {
        return Err(QueryError::Cancelled);
    }
    if !chunk.is_empty() {
        append_chunk(
            segment,
            segment_id,
            plan,
            timestamp_column,
            &mut chunk,
            output,
        )?;
    }
    Ok(())
}

fn append_chunk(
    segment: &Segment,
    segment_id: i64,
    plan: &Plan,
    timestamp_column: &str,
    chunk: &mut Vec<(u64, Row)>,
    output: &mut impl FnMut(EventDataRow) -> Result<(), QueryError>,
) -> Result<(), QueryError> {
    let dictionary = streaming_chunk_dictionary(segment, chunk)?;
    for (ordinal, row) in chunk.drain(..) {
        validate_row_dictionary(&row, &dictionary)?;
        if !plan.matches(&row, &dictionary) {
            continue;
        }
        let Some(Cell::Ts(at)) = row.get(timestamp_column) else {
            continue;
        };
        let identity = row_key::identity(plan.type_id, &row).map_err(|error| {
            QueryError::Unreadable(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error,
            )))
        })?;
        let mut values = Map::new();
        for field in &plan.fields {
            values.insert(
                field.name.clone(),
                field
                    .column
                    .and_then(|name| row.get(name))
                    .map_or(Ok(Value::Null), |value| cell(value, &dictionary))?,
            );
        }
        output(EventDataRow {
            segment_id,
            type_id: plan.type_id,
            row_ordinal: ordinal,
            timestamp: *at,
            identity,
            values,
        })?;
    }
    Ok(())
}
