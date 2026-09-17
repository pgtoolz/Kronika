//! Structured stderr logging for the collector.
//!
//! `logfmt` renders fields; `events` defines collection and WAL diagnostics;
//! `process` reads the collector's own CPU and memory usage in local mode.
//! Each enabled event is one stderr line. Segment announcements stay on stdout.

mod events;
mod logfmt;
mod process;

use std::sync::OnceLock;
use std::time::Duration;

pub(crate) use events::{
    layout_id, log_collection_failure, log_collection_finish, log_collection_start,
    log_count_degraded, log_flush_summary, log_journal_append, section_name, summary_rows,
};
pub(crate) use logfmt::{LogField, field, render_log_line};
pub(crate) use process::{configure_process_diagnostics, peak_rss_kib, process_cpu_ticks};

/// Ordered from the most severe event to the most verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }
}

/// Read `KRONIKA_LOG_LEVEL` for configuration validation. Unset or non-Unicode
/// values default to info; an unrecognized level returns `None`.
pub(crate) fn log_level_from_env() -> Option<LogLevel> {
    let Ok(value) = std::env::var("KRONIKA_LOG_LEVEL") else {
        return Some(LogLevel::Info);
    };
    LogLevel::parse(&value)
}

pub(crate) fn log_event(level: LogLevel, action: &'static str, fields: &[LogField<'_>]) {
    // Cache the output threshold on first use. Configuration validation above
    // reads the environment independently, before collection starts.
    static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();
    let threshold = LOG_LEVEL.get_or_init(|| log_level_from_env().unwrap_or(LogLevel::Info));
    if level <= *threshold {
        let line = render_log_line(level, action, fields);
        eprintln!("{line}");
    }
}

pub(crate) const fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}

#[cfg(test)]
#[path = "tests/logging.rs"]
mod tests;
