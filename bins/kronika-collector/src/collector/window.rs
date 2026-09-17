//! Collect OS/log windows and retain their rows through a journal-full retry.

use anyhow::Result;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_source_os::{OsScope, ProcFs};
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::SectionBuffers;
use std::path::PathBuf;
use std::time::Instant;

use super::WindowWriter;
use crate::cgroup_discovery::CgroupPass;
use crate::clock::collection_timestamp_after;
use crate::instance_metadata::push_instance_metadata;
use crate::log_sources::{LogRows, LogSources, push_log_sources};
use crate::logging::{LogLevel, field, log_event};
use crate::os_sources::{OsTick, collect_os_sources, push_os_sources};
use crate::pg_sources::push_pg_settings;
use crate::scheduler::DueSet;
use crate::segments::{
    append_window_and_maybe_close, close_open_segment, encode_window, report_written,
};

pub(super) struct BufferedWindow {
    pub(super) buffers: SectionBuffers,
    pub(super) pending_users: Vec<(u8, u32)>,
    pub(super) pending_cgroup_context: Option<OsCgroupContextV2>,
}

#[derive(Default)]
pub(crate) struct PendingWindowOutcome {
    pub(crate) written: Vec<PathBuf>,
    pub(crate) accepted: bool,
    pub(crate) appended: bool,
}

#[derive(Debug)]
struct BufferFailure;

impl WindowWriter<'_> {
    /// Admit log batches one at a time; the first also carries the due OS sources.
    pub(super) fn collect_os_and_logs(
        &mut self,
        due: &DueSet,
        opening_settings: &[SettingsRow],
        logs: &mut LogSources,
        already_appended: bool,
        cgroup_pass: Option<&CgroupPass>,
    ) -> Result<Vec<PathBuf>> {
        let mut first_window = true;
        let mut last_ts = None;
        let mut written = Vec::new();
        let mut appended = already_appended;
        let completed = logs.collect(due, |rows| {
            let batch_due = if first_window {
                first_window = false;
                due.clone()
            } else {
                DueSet::logs()
            };
            let Some(ts) = collection_timestamp_after(last_ts) else {
                return Ok(false);
            };
            last_ts = Some(ts);
            let outcome =
                self.append_window(&batch_due, rows, opening_settings, ts, cgroup_pass)?;
            written.extend(outcome.written);
            appended |= outcome.appended;
            Ok(outcome.accepted)
        })?;

        // No log batch carried the original due set, so collect the OS snapshot as
        // its own ordinary window.
        if first_window {
            let Some(ts) = collection_timestamp_after(last_ts) else {
                return Ok(written);
            };
            let outcome =
                self.append_window(due, &LogRows::default(), opening_settings, ts, cgroup_pass)?;
            written.extend(outcome.written);
            appended |= outcome.appended;
        }

        // A forced cycle closes once, after all of its incremental log batches.
        if completed && appended && due.forced() && !self.segment.is_empty() {
            let dest = close_open_segment(self.journal, self.owner, self.segment, "forced")?;
            self.sched.mark_segment_opened();
            report_written(&dest, "forced");
            written.push(dest);
        }
        Ok(written)
    }

    /// Rebuild retained rows with the new dictionary if the journal fills up.
    pub(crate) fn append_window(
        &mut self,
        due: &DueSet,
        log_rows: &LogRows,
        opening_settings: &[SettingsRow],
        ts: i64,
        cgroup_pass: Option<&CgroupPass>,
    ) -> Result<PendingWindowOutcome> {
        let mut outcome = PendingWindowOutcome::default();
        let mut attempt_due = if self.segment.is_empty() {
            self.sched.recollection_due(due, Instant::now())
        } else {
            due.clone()
        };
        for attempt in 0..2 {
            let fresh_segment = self.segment.is_empty();
            let includes_settings =
                !self.segment.pg_settings_present && !opening_settings.is_empty();
            let buffered =
                match self.buffer_window(&attempt_due, log_rows, opening_settings, ts, cgroup_pass)
                {
                    Ok(Some(buffered)) => buffered,
                    Ok(None) => {
                        outcome.accepted = true;
                        return Ok(outcome);
                    }
                    Err(BufferFailure) => {
                        if fresh_segment {
                            anyhow::bail!(
                                "buffer the collection window after updating segment state"
                            );
                        }
                        return Ok(outcome);
                    }
                };
            let flushed = match encode_window(buffered.buffers, &self.segment.interner) {
                Ok(flushed) => flushed,
                Err(err) => {
                    log_event(
                        LogLevel::Error,
                        "window_encode_failure",
                        &[field("error", format!("{err:#}"))],
                    );
                    if fresh_segment {
                        return Err(err.context("encode the collection window"));
                    }
                    return Ok(outcome);
                }
            };
            match append_window_and_maybe_close(
                self.journal,
                self.owner,
                self.config,
                self.segment,
                ts,
                false,
                &flushed,
            ) {
                Ok(finished) => {
                    let retry = finished.iter().any(|(_, reason)| *reason == "journal-full");
                    for (dest, reason) in finished {
                        self.sched.mark_segment_opened();
                        report_written(&dest, reason);
                        outcome.written.push(dest);
                    }
                    if retry {
                        anyhow::ensure!(
                            attempt == 0,
                            "a fresh segment unexpectedly requested another pre-append close"
                        );
                        // Rebuild from owned logical rows. Section buffers and
                        // dictionary ids belong to the segment that just closed.
                        attempt_due = self.sched.recollection_due(due, Instant::now());
                        continue;
                    }
                    if includes_settings && !self.segment.is_empty() {
                        self.segment.pg_settings_present = true;
                    }
                    if !self.segment.is_empty() {
                        self.segment
                            .user_names
                            .confirm_written(&buffered.pending_users);
                        if let Some(context) = buffered.pending_cgroup_context {
                            self.segment.cgroup_context = Some(context);
                        }
                    }
                    outcome.accepted = true;
                    outcome.appended = true;
                    return Ok(outcome);
                }
                Err(failure) => {
                    let (close_failed, err) = failure.into_parts();
                    log_event(
                        LogLevel::Error,
                        "window_append_failure",
                        &[field("error", format!("{err:#}"))],
                    );
                    return if close_failed {
                        Err(err.context("close the segment for the collection window"))
                    } else if fresh_segment {
                        Err(err.context("append the collection window to the journal"))
                    } else {
                        Ok(outcome)
                    };
                }
            }
        }
        anyhow::bail!("a retained collection window exhausted its append attempts")
    }

    fn buffer_window(
        &mut self,
        due: &DueSet,
        log_rows: &LogRows,
        opening_settings: &[SettingsRow],
        ts: i64,
        cgroup_pass: Option<&CgroupPass>,
    ) -> std::result::Result<Option<BufferedWindow>, BufferFailure> {
        let mut buffers = SectionBuffers::new();
        if self.segment.is_empty()
            && let Err(err) = push_instance_metadata(
                &mut buffers,
                &mut self.segment.interner,
                self.in_container,
                self.config,
                ts,
            )
        {
            log_buffer_failure(&err);
            return Err(BufferFailure);
        }

        let mut os = self.process_io.as_mut().map(|process_io| {
            let fs = ProcFs::from_env();
            collect_os_sources(
                &fs,
                process_io,
                &mut self.segment.interner,
                &mut self.segment.user_names,
                &OsTick {
                    scope: OsScope::Host.as_u8(),
                    ts,
                    in_container: self.in_container,
                    due,
                    cgroup_pass,
                },
            )
        });
        let pending_cgroup_context = os
            .as_mut()
            .and_then(|os| os.deduplicate_context(self.segment.cgroup_context.as_ref()));
        let settings = if self.segment.pg_settings_present {
            &[]
        } else {
            opening_settings
        };
        if let Err(err) = push_pg_settings(&mut buffers, &mut self.segment.interner, settings)
            .and_then(|()| {
                os.as_ref()
                    .map_or(Ok(()), |os| push_os_sources(&mut buffers, os))
            })
            .and_then(|()| push_log_sources(&mut buffers, &mut self.segment.interner, log_rows))
        {
            log_buffer_failure(&err);
            return Err(BufferFailure);
        }
        if buffers.is_empty() {
            return Ok(None);
        }
        Ok(Some(BufferedWindow {
            buffers,
            pending_cgroup_context,
            pending_users: os
                .as_ref()
                .map_or_else(Vec::new, |os| os.pending_users().to_vec()),
        }))
    }
}

pub(super) fn log_buffer_failure(err: &anyhow::Error) {
    log_event(
        LogLevel::Error,
        "window_buffer_failure",
        &[field("error", format!("{err:#}"))],
    );
}
