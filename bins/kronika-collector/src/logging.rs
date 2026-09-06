//! Structured stderr logging for the collector.
//!
//! Events render as one logfmt line per call: a fixed `kronika-collector`
//! prefix, then `level` and `action`, then caller fields. `KRONIKA_LOG_LEVEL`
//! (read once) gates output; the stdout segment announcements stay in `main`.
//! The domain helpers below name the collection, its `type_id`, and its layout
//! consistently so operators can filter by any of them.

use std::fmt::{Display, Write as _};
use std::sync::OnceLock;
use std::time::Duration;

use kronika_writer::FlushSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Error => 1,
            Self::Warn => 2,
            Self::Info => 3,
            Self::Debug => 4,
            Self::Trace => 5,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }
}

pub(crate) struct LogField<'a> {
    key: &'static str,
    value: LogValue<'a>,
}

pub(crate) enum LogValue<'a> {
    Str(&'a str),
    Display(&'a dyn Display),
    Owned(String),
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    U128(u128),
    Usize(usize),
}

pub(crate) trait IntoLogValue<'a> {
    fn into_log_value(self) -> LogValue<'a>;
}

impl<'a, T: Display> IntoLogValue<'a> for &'a T {
    fn into_log_value(self) -> LogValue<'a> {
        LogValue::Display(self)
    }
}

impl<'a> IntoLogValue<'a> for &'a str {
    fn into_log_value(self) -> LogValue<'a> {
        LogValue::Str(self)
    }
}

impl<'a> IntoLogValue<'a> for &'a dyn Display {
    fn into_log_value(self) -> LogValue<'a> {
        LogValue::Display(self)
    }
}

impl IntoLogValue<'static> for String {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::Owned(self)
    }
}

impl IntoLogValue<'static> for std::path::Display<'_> {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::Owned(self.to_string())
    }
}

impl IntoLogValue<'static> for bool {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::Bool(self)
    }
}

impl IntoLogValue<'static> for i32 {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::I32(self)
    }
}

impl IntoLogValue<'static> for i64 {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::I64(self)
    }
}

impl IntoLogValue<'static> for u32 {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::U32(self)
    }
}

impl IntoLogValue<'static> for u64 {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::U64(self)
    }
}

impl IntoLogValue<'static> for u128 {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::U128(self)
    }
}

impl IntoLogValue<'static> for usize {
    fn into_log_value(self) -> LogValue<'static> {
        LogValue::Usize(self)
    }
}

fn current_log_level() -> LogLevel {
    static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();
    *LOG_LEVEL.get_or_init(|| log_level_from_env().unwrap_or(LogLevel::Info))
}

/// The level named by `KRONIKA_LOG_LEVEL`, or `None` when it names nothing
/// known. An unset variable reads as [`LogLevel::Info`].
pub(crate) fn log_level_from_env() -> Option<LogLevel> {
    let Ok(value) = std::env::var("KRONIKA_LOG_LEVEL") else {
        return Some(LogLevel::Info);
    };
    parse_log_level_value(&value)
}

fn parse_log_level_value(value: &str) -> Option<LogLevel> {
    LogLevel::parse(&value.trim().to_ascii_lowercase())
}

fn log_enabled(level: LogLevel) -> bool {
    level.rank() <= current_log_level().rank()
}

pub(crate) fn log_event(level: LogLevel, action: &'static str, fields: &[LogField<'_>]) {
    if log_enabled(level) {
        emit_log_event(level, action, fields);
    }
}

fn emit_log_event(level: LogLevel, action: &'static str, fields: &[LogField<'_>]) {
    let line = render_log_line(level, action, fields);
    eprintln!("{line}");
}

pub(crate) fn render_log_line(
    level: LogLevel,
    action: &'static str,
    fields: &[LogField<'_>],
) -> String {
    let mut line = String::from("kronika-collector");
    push_log_field(&mut line, "level", level.as_str());
    push_log_field(&mut line, "action", action);
    for field in fields {
        push_log_field_value(&mut line, field.key, &field.value);
    }
    line
}

fn push_log_field(line: &mut String, key: &str, value: &str) {
    line.push(' ');
    line.push_str(key);
    line.push('=');
    push_log_value(line, value);
}

fn push_log_field_value(line: &mut String, key: &str, value: &LogValue<'_>) {
    let mut rendered = String::new();
    match value {
        LogValue::Str(value) => {
            push_log_field(line, key, value);
            return;
        }
        LogValue::Display(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::Owned(value) => rendered.push_str(value),
        LogValue::Bool(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::I32(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::I64(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::U32(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::U64(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::U128(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
        LogValue::Usize(value) => {
            let _ = write!(&mut rendered, "{value}");
        }
    }
    push_log_field(line, key, &rendered);
}

fn push_log_value(line: &mut String, value: &str) {
    let plain = !value.is_empty()
        && value.chars().all(|ch| {
            !ch.is_whitespace() && !ch.is_control() && ch != '=' && ch != '"' && ch != '\\'
        });
    if plain {
        line.push_str(value);
        return;
    }
    line.push('"');
    for ch in value.chars() {
        match ch {
            '"' => line.push_str("\\\""),
            '\\' => line.push_str("\\\\"),
            '\n' => line.push_str("\\n"),
            '\r' => line.push_str("\\r"),
            '\t' => line.push_str("\\t"),
            _ if ch.is_control() => {
                line.push_str("\\u{");
                let _ = write!(line, "{:x}", ch as u32);
                line.push('}');
            }
            _ => line.push(ch),
        }
    }
    line.push('"');
}

pub(crate) fn field<'a>(key: &'static str, value: impl IntoLogValue<'a>) -> LogField<'a> {
    LogField {
        key,
        value: value.into_log_value(),
    }
}

pub(crate) const fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}

pub(crate) const fn layout_id(type_id: u32) -> u32 {
    type_id % 1_000
}

pub(crate) fn section_name(type_id: u32) -> &'static str {
    kronika_registry::section_name(type_id).unwrap_or("unknown")
}

/// The three identity fields shared by every section log: registry name,
/// raw `type_id`, and its layout suffix.
pub(crate) fn section_fields(type_id: u32) -> [LogField<'static>; 3] {
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

/// Peak resident set size of this process, kibibytes.
///
/// `VmHWM` is the kernel's own watermark, so it needs no sampling of our own.
/// A kernel that does not report it leaves the field at zero rather than
/// making the caller decide what to print.
pub(crate) fn peak_rss_kib() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
