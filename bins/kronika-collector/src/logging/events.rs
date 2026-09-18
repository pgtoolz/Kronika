//! Stable fields for collection, encoding and WAL append events.

use std::fmt::Display;
use std::time::Duration;

use kronika_writer::FlushSummary;

use super::{LogField, LogLevel, duration_ms, field, log_event};

pub(crate) const fn layout_id(type_id: u32) -> u32 {
    type_id % 1_000
}

pub(crate) fn section_name(type_id: u32) -> &'static str {
    kronika_registry::section_name(type_id).unwrap_or("unknown")
}

/// The three identity fields shared by every section log: registry name,
/// raw `type_id`, and its layout suffix.
fn section_fields(type_id: u32) -> [LogField<'static>; 3] {
    [
        field("collection", section_name(type_id)),
        field("type_id", type_id),
        field("layout_id", layout_id(type_id)),
    ]
}

pub(crate) fn log_collection_start(type_id: u32, source: &str) {
    let [collection, type_id, layout_id] = section_fields(type_id);
    log_event(
        LogLevel::Debug,
        "collection_start",
        &[collection, type_id, layout_id, field("source", source)],
    );
}

pub(crate) fn log_collection_finish(type_id: u32, source: &str, rows: usize, elapsed: Duration) {
    let [collection, type_id, layout_id] = section_fields(type_id);
    log_event(
        LogLevel::Debug,
        "collection_finish",
        &[
            collection,
            type_id,
            layout_id,
            field("source", source),
            field("rows", rows),
            field("elapsed_ms", duration_ms(elapsed)),
        ],
    );
}

pub(crate) fn log_collection_failure(
    type_id: u32,
    source: &str,
    err: &(dyn Display + '_),
    elapsed: Duration,
) {
    let [collection, type_id, layout_id] = section_fields(type_id);
    log_event(
        LogLevel::Error,
        "collection_failure",
        &[
            collection,
            type_id,
            layout_id,
            field("source", source),
            field("error", err),
            field("elapsed_ms", duration_ms(elapsed)),
        ],
    );
}

pub(crate) fn log_count_degraded(
    type_id: u32,
    source: &'static str,
    reason: &'static str,
    count: usize,
) {
    let [collection, type_id, layout_id] = section_fields(type_id);
    log_event(
        LogLevel::Warn,
        "collection_degraded",
        &[
            collection,
            type_id,
            layout_id,
            field("source", source),
            field("reason", reason),
            field("count", count),
        ],
    );
}

pub(crate) fn summary_rows(summary: &FlushSummary) -> u64 {
    let mut rows = 0_u64;
    for section in &summary.sections {
        rows = rows.saturating_add(u64::from(section.rows));
    }
    rows
}

pub(crate) fn log_flush_summary(summary: &FlushSummary, elapsed: Duration) {
    log_event(
        LogLevel::Debug,
        "window_encoded",
        &[
            field("sections", summary.sections.len()),
            field("section_rows", summary_rows(summary)),
            field("part_bytes", summary.part_bytes),
            field("elapsed_ms", duration_ms(elapsed)),
        ],
    );
    for section in &summary.sections {
        let [collection, type_id, layout_id] = section_fields(section.type_id);
        log_event(
            LogLevel::Debug,
            "section_encoded",
            &[
                collection,
                type_id,
                layout_id,
                field("section_rows", section.rows),
                field("encoded_bytes", section.body_bytes),
                field("part_bytes", summary.part_bytes),
            ],
        );
    }
}

pub(crate) fn log_journal_append(
    summary: &FlushSummary,
    part_offset: usize,
    part_len: usize,
    journal_bytes_before: usize,
    journal_bytes_after: usize,
    elapsed: Duration,
    retry_after_write: bool,
) {
    log_event(
        LogLevel::Debug,
        "journal_append_finish",
        &[
            field("part_offset", part_offset),
            field("part_len", part_len),
            field("part_bytes", summary.part_bytes),
            field("sections", summary.sections.len()),
            field("section_rows", summary_rows(summary)),
            field("journal_bytes_before", journal_bytes_before),
            field("journal_bytes_after", journal_bytes_after),
            field("retry_after_write", retry_after_write),
            field("elapsed_ms", duration_ms(elapsed)),
        ],
    );
}
