//! Representation of one allowlisted per-segment index series.

use kronika_index::{
    DERIVED_HEALTH_TYPE_ID, FindingBlock, FindingKind, INSTANCE_METADATA_TYPE_ID,
    INSTANCE_METADATA_V1_TYPE_ID, OS_PSI_TYPE_ID, SeriesBlock, series_keys_for_sections,
};
use kronika_reader::SegmentKind;
use kronika_registry::{contract, logical_section_name, section_implementation};
use serde_json::{Value, json};

use crate::dataset::{DatasetSegment, QueryDataset};
use crate::index_provider::{IndexProvider, IndexResource};
use crate::render::record;
use crate::{IndexRequest, QueryError, QuerySink, Window};

pub(crate) struct PreparedIndex {
    segment: DatasetSegment,
    logical_name: String,
    resource: IndexResource,
}

pub(crate) fn prepare(
    dataset: &dyn QueryDataset,
    indexes: &dyn IndexProvider,
    request: &IndexRequest,
) -> Result<PreparedIndex, QueryError> {
    let listing = dataset.segment(request.segment_id)?;
    let segment = listing
        .segments
        .into_iter()
        .next()
        .ok_or(QueryError::NoSuchSegment)?;
    let keys = series_keys_for_sections(segment.sections(), &request.section);
    if keys.is_empty() {
        return Err(QueryError::NoSuchSection);
    }
    let resource = indexes.load(&segment, &request.section, &keys)?;
    validate_checksum(segment.kind(), resource.index.checksum)?;
    Ok(PreparedIndex {
        segment,
        logical_name: request.section.clone(),
        resource,
    })
}

impl PreparedIndex {
    pub(crate) const fn kind(&self) -> SegmentKind {
        self.segment.kind()
    }

    pub(crate) const fn checksum(&self) -> Option<u32> {
        self.resource.index.checksum
    }

    pub(crate) fn stream(self, sink: &mut dyn QuerySink) -> Result<(), QueryError> {
        if sink.cancelled()
            || !sink.record(record(json!({
                "record": "index",
                "segment": segment_value(&self.segment),
                "logical_name": self.logical_name,
                "checksum": self.resource.index.checksum.map(|value| format!("{value:08x}")),
            }))?)
        {
            return Ok(());
        }
        stream_series(&self.logical_name, self.resource, None, sink).map(|_connected| ())
    }
}

pub(crate) fn stream_series(
    logical_name: &str,
    resource: IndexResource,
    window: Option<Window>,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    {
        for block in resource.index.blocks {
            if let SeriesBlock::Findings(block) = block {
                let finding_logical_name = if block.type_id == 0 {
                    "health"
                } else {
                    logical_section_name(block.type_id).ok_or(QueryError::NoSuchSection)?
                };
                if !stream_findings(finding_logical_name, block, window, sink)? {
                    return Ok(false);
                }
                continue;
            }
            if sink.cancelled()
                || !sink.record(record(json!({
                    "record": "layout",
                    "layout": block_layout(logical_name, &block)?,
                }))?)
            {
                return Ok(false);
            }
            let health_series = match &block {
                SeriesBlock::OsHealth(_) => Some("os_health"),
                SeriesBlock::OverallHealth(_) => Some("overall_health"),
                SeriesBlock::PostgresHealth(_) => Some("postgres_health"),
                _ => None,
            };
            match block {
                SeriesBlock::OsHealth(points)
                | SeriesBlock::OverallHealth(points)
                | SeriesBlock::PostgresHealth(points) => {
                    let Some(series) = health_series else {
                        return Err(QueryError::Unreadable(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "health block has no series name",
                        ))));
                    };
                    for point in points.into_iter().filter(|point| {
                        window.is_none_or(|window| window.contains(point.timestamp))
                    }) {
                        if sink.cancelled()
                            || !sink.record(record(json!({
                                "record": "point",
                                "series": series,
                                "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": {},
                                "value": point.value,
                            }))?)
                        {
                            return Ok(false);
                        }
                    }
                }
                SeriesBlock::PgTransactions { type_id, points } => {
                    for point in points.into_iter().filter(|point| {
                        window.is_none_or(|window| window.contains(point.timestamp))
                    }) {
                        if sink.cancelled()
                            || !sink.record(record(json!({
                                "record": "point",
                                "series": "transactions_per_second",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": { "datid": point.datid },
                                "value": point.value,
                            }))?)
                        {
                            return Ok(false);
                        }
                    }
                }
                SeriesBlock::PgActiveBackends { type_id, points } => {
                    for point in points.into_iter().filter(|point| {
                        window.is_none_or(|window| window.contains(point.timestamp))
                    }) {
                        if sink.cancelled()
                            || !sink.record(record(json!({
                                "record": "point",
                                "series": "active_backends",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": {},
                                "value": point.count,
                            }))?)
                        {
                            return Ok(false);
                        }
                    }
                }
                SeriesBlock::Findings(_) => return Err(QueryError::NoSuchSection),
            }
        }
        Ok(true)
    }
}

fn stream_findings(
    logical_name: &str,
    mut block: FindingBlock,
    window: Option<Window>,
    sink: &mut dyn QuerySink,
) -> Result<bool, QueryError> {
    if let Some(window) = window {
        let omitted_may_intersect = block.truncated
            && block
                .findings
                .last()
                .is_none_or(|last| window.to.is_none_or(|to| to >= last.timestamp));
        block
            .findings
            .retain(|finding| window.contains(finding.timestamp));
        block.total_hits = u32::try_from(block.findings.len()).unwrap_or(u32::MAX);
        block.truncated = omitted_may_intersect;
    }
    if sink.cancelled()
        || !sink.record(record(json!({
            "record": "findings",
            "logical_name": logical_name,
            "type_id": block.type_id.to_string(),
            "total_hits": block.total_hits,
            "truncated": block.truncated,
        }))?)
    {
        return Ok(false);
    }
    for finding in block.findings {
        if sink.cancelled() {
            return Ok(false);
        }
        let mut value = json!({
            "record": "finding",
            "logical_name": logical_name,
            "kind": finding_kind(finding.kind),
            "type_id": block.type_id.to_string(),
            "field_ordinal": finding.field_ordinal,
            "row_ordinal": finding.row_ordinal,
            "ts": finding.timestamp.to_string(),
        });
        if let Some(category) = finding.category {
            value["category"] = category.into();
        }
        if !sink.record(record(value)?) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn segment_value(segment: &DatasetSegment) -> Value {
    let cursor = segment.active_position().map(|wal_position| {
        json!({
            "segment_id": segment.id().to_string(),
            "wal_position": wal_position.to_string(),
        })
    });
    json!({
        "id": segment.id().to_string(),
        "kind": match segment.kind() {
            SegmentKind::Finished => "finished",
            SegmentKind::Active => "active",
        },
        "min_ts": segment.min_ts().to_string(),
        "max_ts": segment.max_ts().to_string(),
        "cursor": cursor,
    })
}

fn validate_checksum(kind: SegmentKind, checksum: Option<u32>) -> Result<(), QueryError> {
    match kind {
        SegmentKind::Finished => {
            checksum.ok_or_else(|| {
                QueryError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "finished index has no validated checksum",
                )))
            })?;
            Ok(())
        }
        SegmentKind::Active if checksum.is_none() => Ok(()),
        SegmentKind::Active => Err(QueryError::Unreadable(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "active index unexpectedly carries a reusable checksum",
        )))),
    }
}

fn block_layout(_logical_name: &str, block: &SeriesBlock) -> Result<Value, QueryError> {
    match block {
        SeriesBlock::OsHealth(_) => Ok(health_layout("os_health")),
        SeriesBlock::OverallHealth(_) => Ok(health_layout("overall_health")),
        SeriesBlock::PostgresHealth(_) => Ok(health_layout("postgres_health")),
        SeriesBlock::PgTransactions { type_id, .. } => section_layout("pg_stat_database", *type_id),
        SeriesBlock::PgActiveBackends { type_id, .. } => {
            section_layout("pg_stat_activity", *type_id)
        }
        SeriesBlock::Findings(_) => Err(QueryError::NoSuchSection),
    }
}

const fn finding_kind(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::KnownBad => "known_bad",
        FindingKind::Spike => "spike",
        FindingKind::Event => "event",
    }
}

pub(crate) fn section_layout(logical_name: &str, type_id: u32) -> Result<Value, QueryError> {
    if type_id == DERIVED_HEALTH_TYPE_ID {
        return Ok(health_layout("os_health"));
    }
    let contract = contract(type_id).ok_or(QueryError::NoSuchSection)?;
    let (identity, columns) = match logical_name {
        "pg_stat_database" => (
            json!(["datid"]),
            json!([{
                "name": "transactions_per_second",
                "type": "f64",
                "class": "gauge",
                "unit": "per_second",
                "nullable": true,
            }]),
        ),
        "pg_stat_activity" => (
            json!([]),
            json!([{
                "name": "active_backends",
                "type": "u32",
                "class": "gauge",
                "unit": "count",
                "nullable": false,
            }]),
        ),
        _ => return Err(QueryError::NoSuchSection),
    };
    Ok(json!({
        "logical_name": logical_name,
        "physical_name": contract.name,
        "type_id": type_id.to_string(),
        "implementation": section_implementation(type_id),
        "identity": identity,
        "columns": columns,
    }))
}

fn health_layout(series: &str) -> Value {
    json!({
        "logical_name": "health",
        "physical_name": format!("derived_{series}"),
        "type_id": DERIVED_HEALTH_TYPE_ID.to_string(),
        "implementation": "kronika",
        "identity": [],
        "columns": [{
            "name": series,
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

#[cfg(test)]
#[path = "index/tests.rs"]
mod tests;
