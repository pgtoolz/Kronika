//! Publish the open segment, reset its journal, and announce completed writes.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use kronika_layout::{FileKind, SegmentAddress, WriterOwner};
use kronika_writer::{Journal, write_segment};

use super::SegmentState;
use crate::logging::{LogLevel, duration_ms, field, log_event, peak_rss_kib};

/// Write the open segment into its first window's canonical UTC path and reset
/// the journal.
pub(crate) fn close_open_segment(
    journal: &mut Journal,
    owner: &WriterOwner,
    segment: &mut SegmentState,
    reason: &'static str,
) -> Result<PathBuf> {
    let segment_id = segment
        .first_id
        .context("writing an open segment requires an appended window")?;
    // The journal is the durable input to this fatal operation and startup
    // recovery. Drop the in-memory dictionaries before final bodies are built.
    *segment = SegmentState::with_seal_seed(segment.seal_seed);
    let address = SegmentAddress::new(segment_id).context("derive the segment UTC address")?;
    let dest = owner.root().diagnostic_file_path(address, FileKind::Zms);
    let journal_bytes = journal.bytes();
    let journal_parts = journal.parts().len();
    let started = Instant::now();
    let log_failure = |stage: &'static str, error: &dyn fmt::Display| {
        log_event(
            LogLevel::Error,
            "segment_close_failure",
            &[
                field("segment_path", dest.display()),
                field("segment_id", segment_id.get()),
                field("reason", reason),
                field("stage", stage),
                field("journal_bytes", journal_bytes),
                field("journal_parts", journal_parts),
                field("elapsed_ms", duration_ms(started.elapsed())),
                field("error", error),
            ],
        );
    };
    let summary = write_segment(journal, owner, address).map_err(|error| {
        log_failure("write", &error);
        anyhow::Error::new(error).context("write the segment")
    })?;
    log_event(
        LogLevel::Info,
        "segment_write_finish",
        &[
            field("segment_path", dest.display()),
            field("segment_id", segment_id.get()),
            field("reason", reason),
            field("sections", summary.sections),
            field("segment_bytes", summary.bytes),
            field("journal_bytes", journal_bytes),
            field("journal_parts", journal_parts),
            field("min_ts", summary.min_ts),
            field("max_ts", summary.max_ts),
            field("elapsed_ms", duration_ms(started.elapsed())),
            field("rss_kib", peak_rss_kib()),
        ],
    );
    journal.reset().map_err(|error| {
        log_failure("journal-reset", &error);
        anyhow::Error::new(error).context("reset the journal after the segment write")
    })?;
    Ok(dest)
}

/// Publish the segment path on stdout and flush it for pipe consumers.
/// A closed output pipe must not turn a completed storage write into a failure.
pub(crate) fn report_written(path: &Path, reason: &str) {
    let mut stdout = std::io::stdout().lock();
    drop(
        writeln!(stdout, "wrote {} reason={reason}", path.display()).and_then(|()| stdout.flush()),
    );
}
