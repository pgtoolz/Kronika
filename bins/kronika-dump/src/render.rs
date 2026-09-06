//! Printing a segment as a table or as JSON.

use std::collections::HashSet;
use std::io::Write;

use kronika_index::SeriesBlock;
use kronika_reader::{Cell, Dictionary, Reader, Resolved, Segment, SegmentRef, StoreWarning};
use kronika_registry::{DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID, contract, section_name};
use serde_json::{Map, Value, json};

use crate::DumpError;

/// A file the scan would not admit.
///
/// It goes to the same stream as everything else under `--json`: a caller that
/// only reads stdout still learns that something was left out.
pub(crate) fn warning(
    output: &mut impl Write,
    json_output: bool,
    warning: &StoreWarning,
) -> Result<(), DumpError> {
    if json_output {
        say(
            output,
            &json!({"kind": "warning", "detail": format!("{warning:?}")}),
        )?;
    } else {
        eprintln!("kronika-dump: set aside {warning:?}");
    }
    Ok(())
}

/// What each section of the segment costs.
pub(crate) fn sizes(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
) -> Result<(), DumpError> {
    let section_bytes: u64 = segment.sections().map(|(_id, section)| section.bytes).sum();
    let captured_bytes = segment.captured_bytes();
    let overhead_bytes = captured_bytes.checked_sub(section_bytes).ok_or_else(|| {
        kronika_reader::ReaderError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("section bodies use {section_bytes} bytes in a {captured_bytes}-byte segment"),
        ))
    })?;
    if json_output {
        let path = segment.source_label();
        say(
            output,
            &json!({
                "kind": "segment",
                "path": path,
                "min_ts": segment.min_ts(),
                "max_ts": segment.max_ts(),
                "windows": segment.window_count(),
                "captured_bytes": captured_bytes,
                "section_bytes": section_bytes,
                "overhead_bytes": overhead_bytes,
            }),
        )?;
        for (type_id, section) in segment.sections() {
            say(
                output,
                &json!({
                    "kind": "section",
                    "path": path,
                    "type_id": type_id,
                    "section": section_name(type_id).unwrap_or("unknown"),
                    "rows": section.rows,
                    "bytes": section.bytes,
                    "share_percent": percent(section.bytes, captured_bytes),
                }),
            )?;
        }
        say(
            output,
            &json!({
                "kind": "overhead",
                "path": path,
                "bytes": overhead_bytes,
                "share_percent": percent(overhead_bytes, captured_bytes),
            }),
        )?;
        return Ok(());
    }
    writeln!(
        output,
        "{}  ts={}..{}  windows={}  captured_bytes={captured_bytes}  section_bytes={section_bytes}  overhead_bytes={overhead_bytes}",
        segment.source_label(),
        segment.min_ts(),
        segment.max_ts(),
        segment.window_count()
    )?;
    for (type_id, section) in segment.sections() {
        writeln!(
            output,
            "  {type_id:<9} {:<22} rows={:<8} bytes={:<10} {}%",
            section_name(type_id).unwrap_or("unknown"),
            section.rows,
            section.bytes,
            percent(section.bytes, captured_bytes)
        )?;
    }
    writeln!(
        output,
        "  {:<9} {:<22} rows={:<8} bytes={:<10} {}%",
        "-",
        "physical overhead",
        "-",
        overhead_bytes,
        percent(overhead_bytes, captured_bytes)
    )?;
    Ok(())
}

/// The small presentation-series index this segment would get.
///
/// # Errors
///
/// Returns a build, reader, or index-encoding error when the segment cannot be
/// summarized exactly.
#[cfg(test)]
pub(crate) fn index(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
) -> Result<(), DumpError> {
    let built = kronika_index::build(segment)?;
    write_index(output, json_output, segment, &built)
}

pub(crate) fn index_from_reader(
    output: &mut impl Write,
    json_output: bool,
    reader: &Reader,
    segment_ref: &SegmentRef,
    segment: &Segment,
) -> Result<(), DumpError> {
    let built = kronika_index::build_from_reader(reader, segment_ref, segment)?;
    write_index(output, json_output, segment, &built)
}

fn write_index(
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
                        say(
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
                        say(
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
                    say(
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
                        say(output, &value)?;
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
        say(
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

/// The rows of one section, with dictionary ids resolved to what they hold.
///
/// # Errors
///
/// Returns the reader's error when the section or the dictionary cannot be
/// decoded.
pub(crate) fn section(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
    type_id: u32,
    limit: usize,
) -> Result<(), DumpError> {
    if matches!(type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID) {
        let dictionary = segment.dictionary()?;
        return dictionary_section(output, json_output, segment, type_id, limit, &dictionary);
    }

    let contract = contract(type_id).ok_or_else(|| kronika_reader::ReaderError::Section {
        type_id,
        source: kronika_registry::CodecError::UnknownType { type_id },
    })?;
    let columns = contract
        .columns
        .iter()
        .map(|column| column.name)
        .collect::<Vec<_>>();
    let total_rows = segment.rows_of(type_id).unwrap_or(0);
    let row_limit = if limit == 0 { usize::MAX } else { limit };
    let mut rows = Vec::with_capacity(if limit == 0 {
        0
    } else {
        limit.min(usize::try_from(total_rows).unwrap_or(usize::MAX))
    });
    segment.visit_rows(type_id, &columns, 0, row_limit, |_ordinal, row| {
        rows.push(row);
        true
    })?;
    let ids = rows
        .iter()
        .flat_map(kronika_reader::Row::iter)
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let dictionary = segment.dictionary_for(&ids)?;
    if !json_output {
        writeln!(
            output,
            "{}  {} ({type_id})  rows={}",
            segment.source_label(),
            section_name(type_id).unwrap_or("unknown"),
            total_rows
        )?;
    }
    for row in &rows {
        if json_output {
            let object: Map<String, Value> = row
                .iter()
                .map(|(name, cell)| ((*name).to_owned(), json_cell(cell, &dictionary)))
                .collect();
            write_json_row(
                output,
                segment.source_label(),
                type_id,
                &Value::Object(object),
            )?;
        } else {
            let body: Vec<String> = row
                .iter()
                .map(|(name, cell)| {
                    let value = show(cell, &dictionary);
                    let shown = value.as_deref().unwrap_or("null");
                    format!("{name}={shown}")
                })
                .collect();
            writeln!(output, "  {}", body.join(" "))?;
        }
    }
    let shown = u64::try_from(rows.len()).unwrap_or(u64::MAX);
    if !json_output && limit != 0 && total_rows > shown {
        writeln!(output, "  … {} more rows", total_rows - shown)?;
    }
    Ok(())
}

fn dictionary_section(
    output: &mut impl Write,
    json_output: bool,
    segment: &Segment,
    type_id: u32,
    limit: usize,
    dictionary: &Dictionary,
) -> Result<(), DumpError> {
    let mut entries: Vec<_> = dictionary
        .entries()
        .filter(|(_id, resolved)| {
            matches!(
                (type_id, *resolved),
                (DICT_STRINGS_TYPE_ID, Resolved::Str(_)) | (DICT_BLOBS_TYPE_ID, Resolved::Blob(_))
            )
        })
        .collect();
    entries.sort_unstable_by_key(|(id, _resolved)| *id);
    if !json_output {
        writeln!(
            output,
            "{}  {} ({type_id})  rows={}",
            segment.source_label(),
            section_name(type_id).unwrap_or("unknown"),
            entries.len()
        )?;
    }
    let shown = if limit == 0 { entries.len() } else { limit };
    for (id, resolved) in entries.iter().take(shown) {
        if json_output {
            let row = dictionary_json(*id, *resolved);
            write_json_row(output, segment.source_label(), type_id, &row)?;
        } else {
            match resolved {
                Resolved::Str(bytes) => {
                    writeln!(output, "  str_id={id} bytes={bytes:?}")?;
                }
                Resolved::Blob(blob) => {
                    writeln!(
                        output,
                        "  str_id={id} stored_bytes={:?} full_len={} truncated={} full_sha256={:?}",
                        blob.stored_bytes, blob.full_len, blob.truncated, blob.full_sha256
                    )?;
                }
            }
        }
    }
    if !json_output && limit != 0 && entries.len() > limit {
        writeln!(output, "  … {} more rows", entries.len() - limit)?;
    }
    Ok(())
}

fn dictionary_json(id: u64, resolved: Resolved<'_>) -> Value {
    match resolved {
        Resolved::Str(bytes) => json!({
            "str_id": id,
            "bytes": bytes,
        }),
        Resolved::Blob(blob) => json!({
            "str_id": id,
            "stored_bytes": blob.stored_bytes,
            "full_len": blob.full_len,
            "truncated": blob.truncated,
            "full_sha256": blob.full_sha256,
        }),
    }
}

fn write_json_row(
    output: &mut impl Write,
    source_label: &str,
    type_id: u32,
    row: &Value,
) -> Result<(), DumpError> {
    say(
        output,
        &json!({
            "kind": "row",
            "path": source_label,
            "type_id": type_id,
            "row": row,
        }),
    )
}

/// One cell as text. A dictionary id becomes what the segment interned under
/// it; a blob that was stored cut says so rather than passing for whole.
fn show(cell: &Cell, dictionary: &Dictionary) -> Option<String> {
    match cell {
        Cell::Null => None,
        Cell::I16(v) => Some(v.to_string()),
        Cell::I32(v) => Some(v.to_string()),
        Cell::I64(v) | Cell::Ts(v) => Some(v.to_string()),
        Cell::U32(v) => Some(v.to_string()),
        Cell::U64(v) => Some(v.to_string()),
        Cell::F64(v) => Some(v.to_string()),
        Cell::Bool(v) => Some(v.to_string()),
        Cell::ListI32(v) => Some(format!("{v:?}")),
        Cell::StrId(id) => Some(match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
            Some(Resolved::Blob(blob)) => {
                let text = String::from_utf8_lossy(blob.stored_bytes);
                if blob.truncated {
                    format!("{text}… (cut, {} bytes in full)", blob.full_len)
                } else {
                    text.into_owned()
                }
            }
            None => format!("<str {id}>"),
        }),
    }
}

/// `part` as a whole percent of `whole`.
///
/// The arithmetic widens rather than saturates: a section is small next to a
/// segment, but a figure that quietly collapsed to 1% would be worse than one
/// that costs a `u128` multiply.
fn percent(part: u64, whole: u64) -> u8 {
    if whole == 0 {
        return 0;
    }
    let part = part.min(whole);
    let scaled = u128::from(part) * 100 + u128::from(whole) / 2;
    u8::try_from(scaled / u128::from(whole)).unwrap_or(100)
}

/// One cell as JSON. Numbers stay numbers so a dump can be filtered on them;
/// a dictionary id becomes the text it stands for.
fn json_cell(cell: &Cell, dictionary: &Dictionary) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::I16(v) => json!(v),
        Cell::I32(v) => json!(v),
        Cell::I64(v) | Cell::Ts(v) => json!(v),
        Cell::U32(v) => json!(v),
        Cell::U64(v) => json!(v),
        Cell::F64(v) => json!(v),
        Cell::Bool(v) => json!(v),
        Cell::ListI32(v) => json!(v),
        Cell::StrId(_id) => show(cell, dictionary).map_or(Value::Null, Value::String),
    }
}

/// One JSON document per line, so a long dump streams.
fn say(output: &mut impl Write, value: &Value) -> Result<(), DumpError> {
    writeln!(output, "{value}").map_err(DumpError::Output)
}

#[cfg(test)]
mod tests;
