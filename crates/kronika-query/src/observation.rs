//! Common recorded clock for metric queries, independent of event timestamps.

use std::collections::HashSet;
use std::ops::Bound;

use kronika_reader::{Cell, Segment};
use kronika_registry::{ColumnClass, Semantics, contract, registry};

use crate::{
    CapturedCatalog, DatasetSegment, PredecessorSelection, QueryDataset, QueryError, SegmentBounds,
    SegmentSelection, Window,
};

/// Find the latest recorded metric sample, never a mixed segment bound or wall time.
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
    let Some(upper) = catalog.ranges().iter().map(|(_, to)| *to).max() else {
        return Ok(None);
    };
    let bounds = SegmentBounds::inclusive(Some(upper), Some(upper));
    let mut latest = None;
    let mut visited = HashSet::new();
    let candidates = catalog.segments(SegmentSelection::new(bounds))?.segments;
    scan(
        dataset,
        candidates,
        Window::default(),
        &mut latest,
        &mut visited,
        cancelled,
    )?;
    if latest.is_none() {
        // Summary-based predecessor selection skips event-only catalogs and
        // establishes a real lower bound, not a per-source final answer.
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        let type_ids = registry()
            .iter()
            .filter(|layout| metric_timestamp(layout.type_id.get()).is_some())
            .map(|layout| layout.type_id.get())
            .collect();
        let candidates = catalog
            .segments(SegmentSelection {
                bounds,
                predecessor: PredecessorSelection::ForLayouts(type_ids),
            })?
            .segments;
        scan(
            dataset,
            candidates,
            Window::default(),
            &mut latest,
            &mut visited,
            cancelled,
        )?;
    }
    if let Some(lower) = latest.filter(|lower| *lower < upper) {
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        // Other mixed segments may have a newer metric sample than the seed.
        let candidates = catalog
            .segments(SegmentSelection::new(SegmentBounds {
                start: Bound::Excluded(lower),
                end: Bound::Included(upper),
            }))?
            .segments;
        scan(
            dataset,
            candidates,
            Window::default(),
            &mut latest,
            &mut visited,
            cancelled,
        )?;
    }
    Ok(latest)
}

/// Find the metric clock inside an already selected inclusive response window.
pub(crate) fn latest_metric_in_window(
    dataset: &dyn QueryDataset,
    segments: &[DatasetSegment],
    window: Window,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<i64>, QueryError> {
    let mut latest = None;
    scan(
        dataset,
        segments.to_vec(),
        window,
        &mut latest,
        &mut HashSet::new(),
        cancelled,
    )?;
    Ok(latest)
}

fn metric_timestamp(type_id: u32) -> Option<&'static str> {
    let layout = contract(type_id)?;
    if crate::source_bit(type_id).is_none()
        || !matches!(
            layout.semantics,
            Semantics::SnapshotFull | Semantics::ConditionalFull | Semantics::Changed
        )
    {
        return None;
    }
    layout
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .map(|column| column.name)
}

fn scan(
    dataset: &dyn QueryDataset,
    mut segments: Vec<DatasetSegment>,
    window: Window,
    latest: &mut Option<i64>,
    visited: &mut HashSet<i64>,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<(), QueryError> {
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    segments.sort_unstable_by_key(|segment| std::cmp::Reverse(segment.max_ts()));
    for descriptor in segments {
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if latest.is_some_and(|latest| descriptor.max_ts() <= latest) {
            break;
        }
        if !visited.insert(descriptor.id()) {
            continue;
        }
        if !descriptor
            .sections()
            .iter()
            .any(|section| section.rows > 0 && metric_timestamp(section.type_id).is_some())
        {
            continue;
        }
        let segment = dataset.open(&descriptor)?;
        if let Some(ts) = latest_segment_metric_observation(&segment, window, cancelled)? {
            *latest = Some(latest.map_or(ts, |previous| previous.max(ts)));
        }
    }
    Ok(())
}

/// Latest actual metric sample in one already opened segment and inclusive window.
/// Event streams and reference refreshes do not advance this clock.
///
/// # Errors
/// Returns a timestamp projection failure or cancellation.
pub fn latest_segment_metric_observation(
    segment: &Segment,
    window: Window,
    cancelled: &(impl Fn() -> bool + ?Sized),
) -> Result<Option<i64>, QueryError> {
    if cancelled() {
        return Err(QueryError::Cancelled);
    }
    let mut latest = None;
    for type_id in segment.type_ids() {
        let Some(timestamp) = metric_timestamp(type_id) else {
            continue;
        };
        segment.visit_rows(type_id, &[timestamp], 0, usize::MAX, |_ordinal, row| {
            if cancelled() {
                return false;
            }
            if let Some(Cell::Ts(ts)) = row.get(timestamp)
                && window.contains(*ts)
            {
                latest = Some(latest.map_or(*ts, |previous: i64| previous.max(*ts)));
            }
            latest != Some(segment.max_ts())
        })?;
        if cancelled() {
            return Err(QueryError::Cancelled);
        }
        if latest == Some(segment.max_ts()) {
            break;
        }
    }
    Ok(latest)
}
