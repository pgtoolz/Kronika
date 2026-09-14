//! Common recorded clock for metric queries, independent of event timestamps.

use kronika_reader::Cell;
use kronika_registry::{ColumnClass, Semantics, contract};

use crate::{CapturedCatalog, QueryDataset, QueryError, SegmentBounds, SegmentSelection};

/// Find an actual collector observation, never a mixed segment bound or wall time.
/// Reference refreshes and event streams do not advance this clock. Empty source
/// buffers leave no rows, so this cannot distinguish an empty poll from a failed one.
pub(crate) fn latest_metric_observation(
    dataset: &dyn QueryDataset,
    catalog: &dyn CapturedCatalog,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<i64>, QueryError> {
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    let mut segments = catalog
        .segments(SegmentSelection::new(SegmentBounds::all()))?
        .segments;
    segments.sort_unstable_by_key(|segment| std::cmp::Reverse(segment.max_ts()));
    let mut latest = None;
    for descriptor in segments {
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if latest.is_some_and(|latest| descriptor.max_ts() <= latest) {
            break;
        }
        let layouts: Vec<_> = descriptor
            .sections()
            .iter()
            .filter_map(|section| {
                let layout = contract(section.type_id)?;
                if section.rows == 0
                    || crate::source_bit(section.type_id).is_none()
                    || !matches!(
                        layout.semantics,
                        Semantics::SnapshotFull | Semantics::ConditionalFull | Semantics::Changed
                    )
                {
                    return None;
                }
                let timestamp = layout
                    .columns
                    .iter()
                    .find(|column| column.class == ColumnClass::Timestamp)?;
                Some((section.type_id, timestamp.name))
            })
            .collect();
        if layouts.is_empty() {
            continue;
        }
        let segment = dataset.open(&descriptor)?;
        for (type_id, timestamp) in layouts {
            segment.visit_rows(type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                if let Some(Cell::Ts(ts)) = row.get(timestamp) {
                    latest = Some(latest.map_or(*ts, |previous: i64| previous.max(*ts)));
                }
                latest != Some(descriptor.max_ts())
            })?;
            if cancelled() {
                return Err(QueryError::Cancelled);
            }
            if latest == Some(descriptor.max_ts()) {
                break;
            }
        }
    }
    Ok(latest)
}
