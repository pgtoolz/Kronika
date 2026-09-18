//! Neighboring recorded timestamps, independent of collection cadence.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::Arc;

use kronika_reader::Cell;
use kronika_registry::{ColumnClass, logical_section_name, registry};

use crate::{
    QueryDataset, QueryError, QuerySink, SegmentBounds, SegmentSelection,
    SnapshotNeighborDirection::{Next, Previous},
    SnapshotNeighborRequest, Window,
};

/// Minimum cursor movement for recorded-snapshot navigation: one second.
pub const SNAPSHOT_NEIGHBOR_MIN_STEP_MICROS: i64 = 1_000_000;
/// Maximum logical sections searched by one neighbor request.
pub const MAX_SNAPSHOT_NEIGHBOR_SECTIONS: usize = 32;

pub(crate) struct PreparedNeighbor {
    dataset: Arc<dyn QueryDataset>,
    request: SnapshotNeighborRequest,
}

pub(crate) fn prepare(
    dataset: Arc<dyn QueryDataset>,
    request: SnapshotNeighborRequest,
) -> Result<PreparedNeighbor, QueryError> {
    if request.sections.is_empty()
        || request.sections.len() > MAX_SNAPSHOT_NEIGHBOR_SECTIONS
        || request.sections.iter().enumerate().any(|(index, name)| {
            name.is_empty() || name.len() > 128 || request.sections[..index].contains(name)
        })
    {
        return Err(QueryError::BadFilter("section".to_owned()));
    }
    if matches!((request.window.from, request.window.to), (Some(from), Some(to)) if from > to) {
        return Err(QueryError::BadFilter("to".to_owned()));
    }
    Ok(PreparedNeighbor { dataset, request })
}

impl PreparedNeighbor {
    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        let selected = self.select(sink)?;
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        let _connected = sink.record(crate::render::record(serde_json::json!({
            "record": "snapshot_neighbor",
            "at": selected.map(|(at, _id)| at.to_string()),
            "segment_id": selected.map(|(_at, id)| id.to_string()),
        }))?);
        Ok(())
    }

    fn select(&self, sink: &dyn QuerySink) -> Result<Option<(i64, i64)>, QueryError> {
        if sink.cancelled() {
            return Err(QueryError::Cancelled);
        }
        let Some(window) = search_window(&self.request) else {
            return Ok(None);
        };
        let layouts = registry()
            .iter()
            .filter(|layout| {
                logical_section_name(layout.type_id.get())
                    .is_some_and(|name| self.request.sections.iter().any(|wanted| wanted == name))
            })
            .filter_map(|layout| {
                layout
                    .columns
                    .iter()
                    .find(|column| column.class == ColumnClass::Timestamp)
                    .map(|column| (layout.type_id.get(), column.name))
            })
            .collect::<BTreeMap<_, _>>();
        if layouts.is_empty() {
            return Ok(None);
        }
        let catalog = self.dataset.catalog()?;
        let mut candidates = catalog
            .segments(SegmentSelection::new(SegmentBounds::inclusive(
                window.from,
                window.to,
            )))?
            .segments;
        drop(catalog);
        candidates.retain(|segment| {
            segment
                .sections()
                .iter()
                .any(|section| section.rows > 0 && layouts.contains_key(&section.type_id))
        });
        match self.request.direction {
            Next => {
                candidates
                    .sort_unstable_by_key(|segment| (segment.min_ts(), Reverse(segment.id())));
            }
            Previous => {
                candidates
                    .sort_unstable_by_key(|segment| Reverse((segment.max_ts(), segment.id())));
            }
        }
        let mut selected: Option<(i64, i64)> = None;
        for candidate in candidates {
            if sink.cancelled() {
                return Err(QueryError::Cancelled);
            }
            // Bounds, not segment identity, establish that no closer sample or
            // equal-time sample with a newer anchor can remain in this direction.
            if selected.is_some_and(|(at, _id)| match self.request.direction {
                Next => candidate.min_ts() > at,
                Previous => candidate.max_ts() < at,
            }) {
                break;
            }
            let segment = self.dataset.open(&candidate)?;
            for section in candidate.sections() {
                let Some(&timestamp) = layouts.get(&section.type_id) else {
                    continue;
                };
                if sink.cancelled() {
                    return Err(QueryError::Cancelled);
                }
                segment.visit_rows(
                    section.type_id,
                    &[timestamp],
                    0,
                    usize::MAX,
                    |_ordinal, row| {
                        if sink.cancelled() {
                            return false;
                        }
                        if let Some(Cell::Ts(at)) = row.get(timestamp)
                            && window.contains(*at)
                            && selected.is_none_or(|(chosen, id)| {
                                (match self.request.direction {
                                    Next => *at < chosen,
                                    Previous => *at > chosen,
                                }) || (*at == chosen && candidate.id() > id)
                            })
                        {
                            selected = Some((*at, candidate.id()));
                        }
                        true
                    },
                )?;
            }
        }
        Ok(selected)
    }
}

fn search_window(request: &SnapshotNeighborRequest) -> Option<Window> {
    let mut window = request.window;
    match request.direction {
        Next => {
            let after = request.at.checked_add(SNAPSHOT_NEIGHBOR_MIN_STEP_MICROS)?;
            window.from = Some(window.from.map_or(after, |from| from.max(after)));
        }
        Previous => {
            let before = request.at.checked_sub(SNAPSHOT_NEIGHBOR_MIN_STEP_MICROS)?;
            window.to = Some(window.to.map_or(before, |to| to.min(before)));
        }
    }
    (!matches!((window.from, window.to), (Some(from), Some(to)) if from > to)).then_some(window)
}

#[cfg(test)]
#[path = "tests/snapshot_neighbor.rs"]
mod tests;
