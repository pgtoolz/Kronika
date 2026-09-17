//! Derived time-series summaries, health points, and finding locators.

use std::io::Write;

use kronika_index::SeriesBlock;
use kronika_reader::{Reader, Segment, SegmentRef};
use serde_json::json;

use super::write_json;
use crate::DumpError;

pub(crate) fn index(
    output: &mut impl Write,
    json_output: bool,
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
) -> Result<(), DumpError> {
    let built = kronika_index::build_from_reader(reader, segment_ref, segment)?;
    write_index(output, json_output, segment, &built)
}

pub(super) fn write_index(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
    built: &kronika_index::Index,
) -> Result<(), DumpError> {
    let path = segment.source_label();
    if json_output {
        for block in &built.blocks {
            match block {
                SeriesBlock::OsHealth(points) => {
                    write_health_points(output, path, "os_health", points)?;
                }
                SeriesBlock::OverallHealth(points) => {
                    write_health_points(output, path, "overall_health", points)?;
                }
                SeriesBlock::PostgresHealth(points) => {
                    write_health_points(output, path, "postgres_health", points)?;
                }
                SeriesBlock::PgTransactions { type_id, points } => {
                    for point in points {
                        write_json(
                            output,
                            &json!({
                                "kind": "point",
                                "path": path,
                                "series": "transactions_per_second",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": { "datid": point.datid },
                                "value": point.value,
                            }),
                        )?;
                    }
                }
                SeriesBlock::PgActiveBackends { type_id, points } => {
                    for point in points {
                        write_json(
                            output,
                            &json!({
                                "kind": "point",
                                "path": path,
                                "series": "active_backends",
                                "type_id": type_id.to_string(),
                                "ts": point.timestamp.to_string(),
                                "identity": {},
                                "value": point.count,
                            }),
                        )?;
                    }
                }
                SeriesBlock::Findings(block) => {
                    write_json(
                        output,
                        &json!({
                            "kind": "findings",
                            "path": path,
                            "type_id": block.type_id.to_string(),
                            "total_hits": block.total_hits,
                            "truncated": block.truncated,
                        }),
                    )?;
                    for finding in &block.findings {
                        let mut value = json!({
                            "kind": "finding",
                            "path": path,
                            "mark": match finding.kind {
                                kronika_index::FindingKind::KnownBad => "known_bad",
                                kronika_index::FindingKind::Spike => "spike",
                                kronika_index::FindingKind::Event => "event",
                            },
                            "type_id": block.type_id.to_string(),
                            "field_ordinal": finding.field_ordinal,
                            "row_ordinal": finding.row_ordinal,
                            "ts": finding.timestamp.to_string(),
                        });
                        if let Some(category) = finding.category {
                            value["category"] = category.into();
                        }
                        write_json(output, &value)?;
                    }
                }
            }
        }
        return Ok(());
    }
    let encoded_bytes = built.encode()?.len();
    let point_count = built.blocks.iter().map(index_block_len).sum::<usize>();
    writeln!(
        output,
        "{path}  blocks={}  points={point_count}  idx_bytes={encoded_bytes}",
        built.blocks.len(),
    )?;
    for block in &built.blocks {
        writeln!(
            output,
            "  {:<28} points={}",
            index_block_name(block),
            index_block_len(block),
        )?;
    }
    Ok(())
}

fn write_health_points(
    output: &mut impl Write,
    path: &str,
    series: &str,
    points: &[kronika_index::HealthPoint],
) -> Result<(), DumpError> {
    for point in points {
        write_json(
            output,
            &json!({
                "kind": "point",
                "path": path,
                "series": series,
                "type_id": "0",
                "ts": point.timestamp.to_string(),
                "identity": {},
                "value": point.value,
            }),
        )?;
    }
    Ok(())
}

const fn index_block_name(block: &SeriesBlock) -> &'static str {
    match block {
        SeriesBlock::OsHealth(_) => "os_health",
        SeriesBlock::OverallHealth(_) => "overall_health",
        SeriesBlock::PostgresHealth(_) => "postgres_health",
        SeriesBlock::PgTransactions { .. } => "transactions_per_second",
        SeriesBlock::PgActiveBackends { .. } => "active_backends",
        SeriesBlock::Findings(_) => "findings",
    }
}

const fn index_block_len(block: &SeriesBlock) -> usize {
    match block {
        SeriesBlock::OsHealth(points)
        | SeriesBlock::OverallHealth(points)
        | SeriesBlock::PostgresHealth(points) => points.len(),
        SeriesBlock::PgTransactions { points, .. } => points.len(),
        SeriesBlock::PgActiveBackends { points, .. } => points.len(),
        SeriesBlock::Findings(block) => block.findings.len(),
    }
}
