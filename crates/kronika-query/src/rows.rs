//! Stable bounded pages over physical row order.

use kronika_reader::{Dictionary, Row, Segment, SegmentKind};
use serde_json::{Value, json};

use super::projection::{Plan, apply_tail, chunk_dictionary, plans};
use super::render::{cell, projected_layout, record};
use super::selection::{active_tail, exact_segment};
use crate::request::{Order, RowsRequest};
use crate::{DatasetSegment, QueryDataset, QueryError, QuerySink, QueryStability};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    segment_id: i64,
    active_position: u64,
    layout_index: usize,
    position: u64,
    binding: u64,
}

pub(crate) struct PreparedRows {
    segment: Segment,
    logical_name: String,
    plans: Vec<Plan>,
    order: Order,
    page_size: usize,
    start: Cursor,
}

pub(crate) fn prepare(
    dataset: &dyn QueryDataset,
    request: RowsRequest,
) -> Result<PreparedRows, QueryError> {
    let binding = binding(&request);
    let parsed = request
        .cursor
        .as_deref()
        .map(Cursor::parse)
        .transpose()?
        .filter(|cursor| {
            cursor.segment_id == request.data.segment.segment_id && cursor.binding == binding
        });
    if request.cursor.is_some() && parsed.is_none() {
        return Err(QueryError::BadCursor);
    }
    let current = exact_segment(dataset, request.data.segment.segment_id)?;
    let segment_ref = pin(dataset, current, parsed)?;
    let tail = active_tail(dataset, &segment_ref, request.data.after)?;
    let segment = dataset.open(&segment_ref)?;
    let mut plans = plans(&segment, &request.data, false)?;
    apply_tail(&mut plans, tail.as_ref())?;
    let active_position = segment.active_position().unwrap_or(0);
    let start = match parsed {
        Some(cursor) => cursor,
        None => match request.order {
            Order::Asc => Cursor {
                segment_id: segment.id(),
                active_position,
                layout_index: 0,
                position: plans[0].start_row,
                binding,
            },
            Order::Desc => {
                let layout_index = plans.len().saturating_sub(1);
                Cursor {
                    segment_id: segment.id(),
                    active_position,
                    layout_index,
                    position: plans[layout_index].rows,
                    binding,
                }
            }
        },
    };
    let Some(plan) = plans.get(start.layout_index) else {
        return Err(QueryError::BadCursor);
    };
    if start.position < plan.start_row
        || start.position > plan.rows
        || start.active_position != active_position
    {
        return Err(QueryError::BadCursor);
    }
    Ok(PreparedRows {
        segment,
        logical_name: request.data.segment.section,
        plans,
        order: request.order,
        page_size: request.page_size,
        start,
    })
}

impl PreparedRows {
    pub(crate) const fn stability(&self) -> QueryStability {
        match self.segment.kind() {
            SegmentKind::Finished => QueryStability::Immutable,
            SegmentKind::Active => QueryStability::Mutable,
        }
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "rows",
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
                "order": self.order.as_str(),
                "page_size": self.page_size.to_string(),
            }))?)
        {
            return Ok(());
        }
        for plan in &self.plans {
            if sink.cancelled() {
                return Ok(());
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
                "layout": projected_layout(&self.logical_name, plan.contract, &fields),
            }))?) {
                return Ok(());
            }
        }

        let page = match self.order {
            Order::Asc => self.asc(sink)?,
            Order::Desc => self.desc(sink)?,
        };
        if !page.connected || sink.cancelled() {
            return Ok(());
        }
        let next = page.next.map(Cursor::encode);
        let _sent = sink.record(record(json!({
            "record": "page",
            "next_cursor": next,
        }))?);
        Ok(())
    }

    fn asc(&self, sink: &mut dyn QuerySink) -> Result<Page, QueryError> {
        let mut page = AscPage {
            emitted: 0,
            next: None,
            connected: true,
        };
        let mut failure = None;
        for layout_index in self.start.layout_index..self.plans.len() {
            if sink.cancelled() {
                page.connected = false;
                break;
            }
            let plan = &self.plans[layout_index];
            if !plan.applies() {
                continue;
            }
            let offset = if layout_index == self.start.layout_index {
                self.start.position
            } else {
                plan.start_row
            };
            let chunk_rows = self.page_size.saturating_add(1);
            let mut chunk = Vec::with_capacity(chunk_rows);
            self.segment.visit_rows(
                plan.type_id,
                &plan.projection,
                offset,
                usize::MAX,
                |ordinal, row| {
                    if sink.cancelled() {
                        page.connected = false;
                        return false;
                    }
                    chunk.push((ordinal, row));
                    if chunk.len() < chunk_rows {
                        return true;
                    }
                    if let Err(error) =
                        self.emit_asc_chunk(plan, layout_index, &mut chunk, &mut page, sink)
                    {
                        failure = Some(error);
                    }
                    page.connected && failure.is_none() && page.next.is_none()
                },
            )?;
            if failure.is_none() && page.connected && page.next.is_none() && !chunk.is_empty() {
                self.emit_asc_chunk(plan, layout_index, &mut chunk, &mut page, sink)?;
            }
            if failure.is_some() || !page.connected || page.next.is_some() {
                break;
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(Page {
            next: page.next,
            connected: page.connected,
        })
    }

    fn emit_asc_chunk(
        &self,
        plan: &Plan,
        layout_index: usize,
        rows: &mut Vec<(u64, Row)>,
        page: &mut AscPage,
        sink: &mut dyn QuerySink,
    ) -> Result<(), QueryError> {
        if sink.cancelled() {
            page.connected = false;
            return Ok(());
        }
        let dictionary = chunk_dictionary(&self.segment, rows)?;
        for (ordinal, row) in rows.drain(..) {
            if sink.cancelled() {
                page.connected = false;
                break;
            }
            if !plan.matches(&row, &dictionary) {
                continue;
            }
            if page.emitted == self.page_size {
                page.next = Some(Cursor {
                    layout_index,
                    position: ordinal,
                    ..self.start
                });
                break;
            }
            page.connected = sink.record(row_record(plan, ordinal, &row, &dictionary)?);
            if !page.connected {
                break;
            }
            page.emitted = page.emitted.saturating_add(1);
        }
        Ok(())
    }

    fn desc(&self, sink: &mut dyn QuerySink) -> Result<Page, QueryError> {
        let mut emitted = 0_usize;
        let mut next = None;
        let mut connected = true;
        let mut failure = None;
        for layout_index in (0..=self.start.layout_index).rev() {
            if sink.cancelled() {
                connected = false;
                break;
            }
            let plan = &self.plans[layout_index];
            if !plan.applies() {
                continue;
            }
            let mut upper = if layout_index == self.start.layout_index {
                self.start.position
            } else {
                plan.rows
            };
            while upper > plan.start_row && connected && next.is_none() {
                let chunk_rows = self.page_size.saturating_add(1);
                let lower = upper.saturating_sub(chunk_rows as u64).max(plan.start_row);
                let limit =
                    usize::try_from(upper - lower).map_err(|_overflow| QueryError::BadCursor)?;
                // Reverse physical order needs only one bounded projected chunk;
                // no complete section or response is retained.
                let mut chunk: Vec<(u64, Row)> = Vec::with_capacity(limit);
                self.segment.visit_rows(
                    plan.type_id,
                    &plan.projection,
                    lower,
                    limit,
                    |ordinal, row| {
                        if sink.cancelled() {
                            connected = false;
                            return false;
                        }
                        chunk.push((ordinal, row));
                        true
                    },
                )?;
                if !connected {
                    break;
                }
                let dictionary = chunk_dictionary(&self.segment, &chunk)?;
                for (ordinal, row) in chunk.into_iter().rev() {
                    if sink.cancelled() {
                        connected = false;
                        break;
                    }
                    if !plan.matches(&row, &dictionary) {
                        continue;
                    }
                    if emitted == self.page_size {
                        next = Some(Cursor {
                            layout_index,
                            position: ordinal.saturating_add(1),
                            ..self.start
                        });
                        break;
                    }
                    match row_record(plan, ordinal, &row, &dictionary) {
                        Ok(bytes) => {
                            connected = sink.record(bytes);
                            emitted = emitted.saturating_add(1);
                        }
                        Err(error) => {
                            failure = Some(error);
                            connected = false;
                            break;
                        }
                    }
                    if !connected {
                        break;
                    }
                }
                upper = lower;
            }
            if failure.is_some() || !connected || next.is_some() {
                break;
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(Page { next, connected })
    }
}

struct Page {
    next: Option<Cursor>,
    connected: bool,
}

struct AscPage {
    emitted: usize,
    next: Option<Cursor>,
    connected: bool,
}

fn pin(
    dataset: &dyn QueryDataset,
    current: DatasetSegment,
    cursor: Option<Cursor>,
) -> Result<DatasetSegment, QueryError> {
    let Some(cursor) = cursor else {
        return Ok(current);
    };
    match current.kind() {
        SegmentKind::Finished if cursor.active_position == 0 => Ok(current),
        SegmentKind::Active => dataset
            .at_active_position(&current, cursor.active_position)
            .map_err(|_error| QueryError::BadCursor),
        SegmentKind::Finished => Err(QueryError::BadCursor),
    }
}

fn row_record(
    plan: &Plan,
    ordinal: u64,
    row: &Row,
    dictionary: &Dictionary,
) -> Result<Vec<u8>, QueryError> {
    let values = plan
        .fields
        .iter()
        .map(|field| {
            field
                .column
                .and_then(|name| row.get(name))
                .map_or(Ok(Value::Null), |value| cell(value, dictionary))
        })
        .collect::<Result<Vec<_>, _>>()?;
    record(json!({
        "record": "row",
        "type_id": plan.type_id.to_string(),
        "ordinal": ordinal.to_string(),
        "values": values,
    }))
}

impl Cursor {
    fn parse(raw: &str) -> Result<Self, QueryError> {
        let fields: Vec<&str> = raw.split(',').collect();
        if fields.len() != 5 {
            return Err(QueryError::BadCursor);
        }
        Ok(Self {
            segment_id: fields[0].parse().map_err(|_error| QueryError::BadCursor)?,
            active_position: fields[1].parse().map_err(|_error| QueryError::BadCursor)?,
            layout_index: fields[2].parse().map_err(|_error| QueryError::BadCursor)?,
            position: fields[3].parse().map_err(|_error| QueryError::BadCursor)?,
            binding: fields[4].parse().map_err(|_error| QueryError::BadCursor)?,
        })
    }

    fn encode(self) -> String {
        format!(
            "{},{},{},{},{}",
            self.segment_id, self.active_position, self.layout_index, self.position, self.binding
        )
    }
}

fn binding(request: &RowsRequest) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_bytes(&mut hash, request.data.segment.section.as_bytes());
    if let Some(type_id) = request.data.type_id {
        hash_bytes(&mut hash, &type_id.to_le_bytes());
    }
    hash_byte(
        &mut hash,
        match request.order {
            Order::Asc => 0,
            Order::Desc => 1,
        },
    );
    for field in &request.data.fields {
        hash_bytes(&mut hash, field.as_bytes());
    }
    for filter in &request.data.filters {
        hash_bytes(&mut hash, filter.column.as_bytes());
        hash_bytes(&mut hash, filter.value.as_bytes());
    }
    if let Some(after) = request.data.after {
        hash_bytes(&mut hash, &after.segment_id.to_le_bytes());
        hash_bytes(&mut hash, &after.wal_position.to_le_bytes());
    }
    hash
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    for byte in len.to_le_bytes().iter().chain(bytes) {
        hash_byte(hash, *byte);
    }
}

fn hash_byte(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
}

#[cfg(test)]
mod tests;
