//! Exact-segment and committed-prefix selection shared by query families.

use kronika_reader::SegmentKind;

use crate::request::ActiveCursor;
use crate::{DatasetSegment, QueryDataset, QueryError};

pub(crate) fn exact_segment(
    dataset: &dyn QueryDataset,
    id: i64,
) -> Result<DatasetSegment, QueryError> {
    dataset
        .segment(id)?
        .segments
        .into_iter()
        .next()
        .ok_or(QueryError::NoSuchSegment)
}

pub(crate) fn active_tail(
    dataset: &dyn QueryDataset,
    current: &DatasetSegment,
    after: Option<ActiveCursor>,
) -> Result<Option<DatasetSegment>, QueryError> {
    let Some(after) = after else {
        return Ok(None);
    };
    if current.kind() != SegmentKind::Active || current.id() != after.segment_id {
        return Err(QueryError::BadCursor);
    }
    dataset
        .at_active_position(current, after.wal_position)
        .map(Some)
        .map_err(|_error| QueryError::BadCursor)
}
