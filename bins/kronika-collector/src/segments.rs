use crate::config::Config;
use crate::logging::{
    LogLevel, duration_ms, field, log_event, log_flush_summary, log_journal_append, peak_rss_kib,
    summary_rows,
};
use anyhow::{Context, Result};
use kronika_layout::{FileKind, SegmentAddress, SegmentId, WriterOwner};
use kronika_writer::{
    FlushedPart, Interner, Journal, JournalConfig, JournalError, SectionBuffers, dict,
    write_segment,
};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::os_sources::UserReferences;

mod open;

pub(crate) use open::open_collector_journal;

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

/// The open (not yet finished) segment: its file name comes from the first
/// window's timestamp, its age from the moment that window was appended.
#[derive(Debug)]
pub(crate) struct SegmentState {
    first_id: Option<SegmentId>,
    opened_at: Option<Instant>,
    interner: Interner,
    users: UserReferences,
    pg_settings_present: bool,
}

impl Default for SegmentState {
    fn default() -> Self {
        Self {
            first_id: None,
            opened_at: None,
            interner: Interner::new(kronika_format::DictLimits::default()),
            users: UserReferences::default(),
            pg_settings_present: false,
        }
    }
}

impl SegmentState {
    pub(crate) const fn is_empty(&self) -> bool {
        self.first_id.is_none()
    }

    pub(crate) const fn interner(&self) -> &Interner {
        &self.interner
    }

    pub(crate) const fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    pub(crate) const fn os_state_mut(&mut self) -> (&mut Interner, &mut UserReferences) {
        (&mut self.interner, &mut self.users)
    }

    pub(crate) fn mark_users_recorded(&mut self, pending: &[(u8, u32)]) {
        self.users.mark_recorded(pending);
    }

    #[cfg(test)]
    pub(crate) fn with_user_snapshot(passwd: kronika_source_os::PasswdSnapshot) -> Self {
        Self {
            users: UserReferences::with_passwd(passwd),
            ..Self::default()
        }
    }

    pub(crate) const fn needs_pg_settings(&self) -> bool {
        !self.pg_settings_present
    }

    pub(crate) const fn mark_pg_settings_present(&mut self) {
        self.pg_settings_present = true;
    }

    /// Register the appended window; the first one opens the segment.
    pub(crate) const fn on_window_appended(&mut self, id: SegmentId, now: Instant) {
        if self.first_id.is_none() {
            self.first_id = Some(id);
            self.opened_at = Some(now);
        }
    }

    /// Whether the open segment has reached `max_age`.
    pub(crate) fn age_expired(&self, now: Instant, max_age: Duration) -> bool {
        self.opened_at
            .is_some_and(|opened| now.duration_since(opened) >= max_age)
    }

    pub(crate) fn time_until_age(&self, now: Instant, max_age: Duration) -> Option<Duration> {
        Some(max_age.saturating_sub(now.saturating_duration_since(self.opened_at?)))
    }

    #[cfg(test)]
    pub(crate) const fn first_ts(&self) -> Option<i64> {
        match self.first_id {
            Some(id) => Some(id.get()),
            None => None,
        }
    }
}

/// Why the open segment must write now, or `None` to keep collecting.
///
/// Forced ticks write immediately; otherwise the raw journal size or segment
/// age closes the segment.
#[derive(Clone, Copy)]
struct CloseConditions {
    forced: bool,
    age_expired: bool,
}

const fn close_reason(
    conditions: CloseConditions,
    journal_bytes: usize,
    max_bytes: u64,
) -> Option<&'static str> {
    if conditions.forced {
        Some("forced")
    } else if journal_bytes as u64 >= max_bytes {
        Some("size")
    } else if conditions.age_expired {
        Some("age")
    } else {
        None
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
    *segment = SegmentState::default();
    let address = SegmentAddress::new(segment_id).context("derive the segment UTC address")?;
    let dest = owner.root().diagnostic_file_path(address, FileKind::Zms);
    let journal_bytes = journal.bytes();
    let journal_parts = journal.parts().len();
    let started = Instant::now();
    let summary = match write_segment(journal, owner, address) {
        Ok(summary) => summary,
        Err(error) => {
            log_close_failure(
                &dest,
                segment_id,
                reason,
                "write",
                journal_bytes,
                journal_parts,
                started.elapsed(),
                &error,
            );
            return Err(anyhow::Error::new(error).context("write the segment"));
        }
    };
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
    if let Err(error) = journal.reset() {
        log_close_failure(
            &dest,
            segment_id,
            reason,
            "journal-reset",
            journal_bytes,
            journal_parts,
            started.elapsed(),
            &error,
        );
        return Err(anyhow::Error::new(error).context("reset the journal after the segment write"));
    }
    Ok(dest)
}

#[allow(
    clippy::too_many_arguments,
    reason = "one close-failure record carries the segment identity, cost, stage, and operating error"
)]
fn log_close_failure(
    dest: &std::path::Path,
    segment_id: SegmentId,
    reason: &'static str,
    stage: &'static str,
    journal_bytes: usize,
    journal_parts: usize,
    elapsed: Duration,
    error: &dyn fmt::Display,
) {
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
            field("elapsed_ms", duration_ms(elapsed)),
            field("error", error),
        ],
    );
}

pub(crate) fn append_window_and_maybe_close(
    journal: &mut Journal,
    owner: &WriterOwner,
    config: &Config,
    segment: &mut SegmentState,
    ts: i64,
    forced: bool,
    flushed: &FlushedPart,
) -> Result<Vec<(PathBuf, &'static str)>, AppendWindowError> {
    let mut finished = Vec::new();
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
    match append_result {
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
            finished.push((
                close_open_segment(journal, owner, segment, "journal-full")
                    .map_err(AppendWindowError::Close)?,
                "journal-full",
            ));
            return Ok(finished);
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
    }
    let now = Instant::now();
    let active_id = journal
        .segment_id()
        .context("a successful journal append must persist SegmentId")?;
    segment.on_window_appended(active_id, now);
    let age = Duration::from_secs(config.segment_max_age_secs);
    if let Some(reason) = close_reason(
        CloseConditions {
            forced,
            age_expired: segment.age_expired(now, age),
        },
        journal.bytes(),
        config.segment_max_bytes,
    ) {
        finished.push((
            close_open_segment(journal, owner, segment, reason)
                .map_err(AppendWindowError::Close)?,
            reason,
        ));
    }
    Ok(finished)
}

#[cfg(test)]
mod tests;
