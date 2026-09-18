//! Keep segment dictionaries and reference rows, append windows, and decide when to seal.
//!
//! A full journal closes the accumulated segment before the incoming window is
//! written. The caller then rebuilds that window against the new dictionary.

mod age;
mod close;
mod open;

pub(crate) use close::{close_open_segment, report_written};
pub(crate) use open::open_collector_journal;

use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use kronika_layout::{SegmentId, WriterOwner};
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_writer::{FlushedPart, Interner, Journal, JournalError, SectionBuffers, dict};

use crate::config::Config;
use crate::logging::{
    LogLevel, duration_ms, field, log_event, log_flush_summary, log_journal_append, summary_rows,
};
use crate::os_sources::SegmentUserNames;

/// The open (not yet finished) segment: its file name comes from the first
/// window's timestamp, its age deadline from the first successful append.
#[derive(Debug)]
pub(crate) struct SegmentState {
    first_id: Option<SegmentId>,
    seal_seed: u64,
    age_deadline: Option<(Instant, Duration)>,
    pub(crate) interner: Interner,
    /// Observed users and the names already written to this segment's WAL.
    pub(crate) user_names: SegmentUserNames,
    /// Set only after a window containing settings reaches the WAL.
    pub(crate) pg_settings_present: bool,
    /// Last context written to the WAL, used to skip unchanged context rows.
    pub(crate) cgroup_context: Option<OsCgroupContextV2>,
}

impl Default for SegmentState {
    fn default() -> Self {
        Self {
            first_id: None,
            seal_seed: 0,
            age_deadline: None,
            interner: Interner::new(kronika_format::DictLimits::default()),
            user_names: SegmentUserNames::default(),
            pg_settings_present: false,
            cgroup_context: None,
        }
    }
}

impl SegmentState {
    pub(crate) fn with_seal_seed(seal_seed: u64) -> Self {
        Self {
            seal_seed,
            ..Self::default()
        }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.first_id.is_none()
    }

    /// Register the first append and retain its monotonic age deadline.
    fn on_window_appended(
        &mut self,
        id: SegmentId,
        now: Instant,
        utc: SystemTime,
        max_age: Duration,
    ) -> Result<()> {
        if self.first_id.is_none() {
            let utc = utc
                .duration_since(UNIX_EPOCH)
                .context("system clock is before unix epoch")?;
            self.age_deadline = Some((now, age::until_next_phase(self.seal_seed, max_age, utc)));
            self.first_id = Some(id);
        }
        Ok(())
    }

    pub(crate) fn age_expired(&self, now: Instant) -> bool {
        self.time_until_age(now)
            .is_some_and(|remaining| remaining.is_zero())
    }

    pub(crate) fn time_until_age(&self, now: Instant) -> Option<Duration> {
        let (started, delay) = self.age_deadline?;
        Some(delay.saturating_sub(now.saturating_duration_since(started)))
    }
}

/// Encode the buffered window into one journal-ready part.
pub(crate) fn encode_window(
    mut buffers: SectionBuffers,
    interner: &Interner,
) -> Result<FlushedPart> {
    let started = Instant::now();
    let dict_sections = dict::encode(interner.window()).context("encode the segment dictionary")?;
    let flushed = buffers
        .flush_with_summary(&dict_sections)
        .context("encode the collection window")?
        .context("a buffered row must yield a part")?;
    log_flush_summary(&flushed.summary, started.elapsed());
    Ok(flushed)
}

/// Append one encoded window; return any segment closed by this attempt.
/// `journal-full` means the window was not appended and must be rebuilt and retried.
pub(crate) fn append_window_and_maybe_close(
    journal: &mut Journal,
    owner: &WriterOwner,
    config: &Config,
    segment: &mut SegmentState,
    ts: i64,
    forced: bool,
    flushed: &FlushedPart,
) -> Result<Vec<(PathBuf, &'static str)>, AppendWindowError> {
    let segment_id = match segment.first_id {
        Some(segment_id) => segment_id,
        None => SegmentId::new(ts).context("collection timestamp is outside the layout range")?,
    };
    let append_started = Instant::now();
    let journal_bytes_before = journal.bytes();
    let mut appended = None;
    let append_result = segment.interner.flush_window(|_window| {
        journal.append(segment_id, &flushed.body).map(|part_ref| {
            appended = Some(part_ref);
        })
    });
    let reason = match append_result {
        Ok(_flushed_entries) => {
            let part_ref = appended.context("a successful journal append must return its part")?;
            log_journal_append(
                &flushed.summary,
                part_ref.offset(),
                part_ref.len(),
                journal_bytes_before,
                journal.bytes(),
                append_started.elapsed(),
                false,
            );
            let now = Instant::now();
            let active_id = journal
                .segment_id()
                .context("a successful journal append must persist SegmentId")?;
            let age = Duration::from_secs(config.segment_max_age_secs);
            segment.on_window_appended(active_id, now, SystemTime::now(), age)?;
            close_reason(
                forced,
                segment.age_expired(now),
                journal.bytes(),
                config.segment_max_bytes,
            )
        }
        Err(JournalError::TooManyParts { max }) if segment.first_id.is_some() => {
            log_event(
                LogLevel::Warn,
                "journal_parts_full",
                &[
                    field("parts", journal.parts().len()),
                    field("max_parts", max),
                ],
            );
            Some("journal-full")
        }
        Err(JournalError::Full { len, max }) if segment.first_id.is_some() => {
            log_event(
                LogLevel::Warn,
                "journal_full",
                &[
                    field("journal_bytes", len),
                    field("journal_max_bytes", max),
                    field("part_bytes", flushed.summary.part_bytes),
                    field("sections", flushed.summary.sections.len()),
                    field("section_rows", summary_rows(&flushed.summary)),
                ],
            );
            Some("journal-full")
        }
        Err(other) => {
            log_event(
                LogLevel::Error,
                "journal_append_failure",
                &[
                    field("part_bytes", flushed.summary.part_bytes),
                    field("sections", flushed.summary.sections.len()),
                    field("section_rows", summary_rows(&flushed.summary)),
                    field("journal_bytes_before", journal_bytes_before),
                    field("error", &other),
                    field("elapsed_ms", duration_ms(append_started.elapsed())),
                ],
            );
            return Err(anyhow::Error::new(other)
                .context("append the part to the journal")
                .into());
        }
    };
    let Some(reason) = reason else {
        return Ok(Vec::new());
    };
    let path =
        close_open_segment(journal, owner, segment, reason).map_err(AppendWindowError::Close)?;
    Ok(vec![(path, reason)])
}

/// Forced closes take priority over size, then age.
const fn close_reason(
    forced: bool,
    age_expired: bool,
    journal_bytes: usize,
    max_bytes: u64,
) -> Option<&'static str> {
    if forced {
        Some("forced")
    } else if journal_bytes as u64 >= max_bytes {
        Some("size")
    } else if age_expired {
        Some("age")
    } else {
        None
    }
}

/// Failure while appending or closing one collection window.
#[derive(Debug)]
pub(crate) enum AppendWindowError {
    /// The segment could not be written or its journal could not be reset.
    Close(anyhow::Error),
    /// The incoming window could not be appended safely.
    Other(anyhow::Error),
}

impl AppendWindowError {
    /// Split the failure into its fatal-close classification and full error.
    pub(crate) fn into_parts(self) -> (bool, anyhow::Error) {
        match self {
            Self::Close(error) => (true, error),
            Self::Other(error) => (false, error),
        }
    }
}

impl fmt::Display for AppendWindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Close(error) | Self::Other(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl From<anyhow::Error> for AppendWindowError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

#[cfg(test)]
#[path = "tests/segments.rs"]
mod tests;
