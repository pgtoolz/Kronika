//! Snapshot cursor encoding and request identity binding.

use kronika_reader::SegmentKind;

use super::{PartitionSource, SnapshotCursor};

use crate::dataset::{DatasetSegment, QueryDataset};
use crate::snapshot::search::StructuredSearch;
use crate::{QueryError, SnapshotRequest};

impl SnapshotCursor {
    pub(super) fn parse(raw: &str) -> Result<Self, QueryError> {
        let fields = raw.split(',').collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(QueryError::BadCursor);
        }
        Ok(Self {
            segment_id: fields[0].parse().map_err(|_error| QueryError::BadCursor)?,
            active_position: fields[1].parse().map_err(|_error| QueryError::BadCursor)?,
            context_index: fields[2].parse().map_err(|_error| QueryError::BadCursor)?,
            ordinal: fields[3].parse().map_err(|_error| QueryError::BadCursor)?,
            binding: fields[4].parse().map_err(|_error| QueryError::BadCursor)?,
        })
    }

    pub(super) fn encode(self) -> String {
        format!(
            "{},{},{},{},{}",
            self.segment_id, self.active_position, self.context_index, self.ordinal, self.binding
        )
    }
}

pub(super) const fn partition_context_index(
    layout_index: usize,
    source: PartitionSource,
    predecessor_count: usize,
) -> usize {
    layout_index * (predecessor_count + 1)
        + match source {
            PartitionSource::Current => 0,
            PartitionSource::Earlier(index) => predecessor_count - index,
        }
}

pub(super) const fn timed_context_index(
    layout_index: usize,
    source_index: usize,
    source_count: usize,
) -> usize {
    layout_index * source_count + source_index
}

pub(super) fn pin(
    dataset: &dyn QueryDataset,
    current: DatasetSegment,
    cursor: Option<SnapshotCursor>,
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

pub(super) fn snapshot_binding(
    request: &SnapshotRequest,
    search: Option<&StructuredSearch>,
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_part(&mut hash, b"segment", &request.segment_id.to_le_bytes());
    hash_part(&mut hash, b"at", &request.at.to_le_bytes());
    if request.latest {
        // Keep legacy anchor cursors valid while binding the opt-in policy.
        hash_part(&mut hash, b"selection", b"latest");
    }
    for section in &request.sections {
        hash_part(&mut hash, b"section", section.as_bytes());
    }
    if let Some(type_id) = request.type_id {
        hash_part(&mut hash, b"type", &type_id.to_le_bytes());
    }
    for field in &request.fields {
        hash_part(&mut hash, b"field", field.as_bytes());
    }
    if let Some(text) = request.text {
        hash_part(&mut hash, b"text", &text.to_le_bytes());
    }
    for filter in &request.filters {
        hash_part(&mut hash, b"filter-column", filter.column.as_bytes());
        hash_part(&mut hash, b"filter-value", filter.value.as_bytes());
    }
    if let Some(search) = search {
        hash_part(&mut hash, b"search", search.canonical().as_bytes());
    }
    hash_part(&mut hash, b"first-match", &[u8::from(request.first_match)]);
    hash_part(&mut hash, b"scope", request.scope.as_str().as_bytes());
    for by in &request.by {
        hash_part(&mut hash, b"by", by.as_bytes());
    }
    if let Some(group) = request.group {
        hash_part(&mut hash, b"group", group.as_str().as_bytes());
    }
    hash_part(
        &mut hash,
        b"direction",
        request.direction.as_str().as_bytes(),
    );
    hash
}

fn hash_part(hash: &mut u64, tag: &[u8], bytes: &[u8]) {
    hash_bytes(hash, tag);
    hash_bytes(hash, bytes);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    for byte in len.to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}
