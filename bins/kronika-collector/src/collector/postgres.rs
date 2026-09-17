//! Admit `PostgreSQL` batches to the WAL, retrying a retained batch after rotation.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use kronika_source_os::{OsScope, ProcFs};
use kronika_source_pg::query::BatchWrite;
use kronika_source_pg::settings::SettingsRow;
use kronika_writer::SectionBuffers;

use super::WindowWriter;
use super::window::{BufferedWindow, log_buffer_failure};
use crate::cgroup_discovery::CgroupPass;
use crate::clock::collection_timestamp_after;
use crate::instance_metadata::push_instance_metadata;
use crate::logging::{LogLevel, field, log_event};
use crate::os_sources::{OsTick, collect_os_sources, push_os_sources};
use crate::pg_sources::query_diagnostics::PgQueryDiagnostics;
use crate::pg_sources::{PgBatch, PgObservation, PgSources, QueryOutcome, push_pg_batch};
use crate::scheduler::DueSet;
use crate::segments::{
    AppendWindowError, append_window_and_maybe_close, encode_window, report_written,
};

#[derive(Default)]
pub(super) struct PgCollectionOutcome {
    pub(super) written: Vec<PathBuf>,
    pub(super) appended: bool,
    pub(super) opening_os_collected: bool,
}

#[derive(Debug)]
pub(crate) enum PgAppendError {
    Rejected,
    Fatal(anyhow::Error),
}

pub(crate) struct PgPendingOutcome {
    pub(crate) written: Vec<PathBuf>,
    pub(crate) write: BatchWrite,
    pub(crate) opening_os_collected: bool,
}

impl WindowWriter<'_> {
    /// Stream due `PostgreSQL` sources into independently admitted WAL parts.
    pub(super) async fn collect_postgres(
        &mut self,
        pg: &mut PgSources,
        due: &DueSet,
        diagnostics: &mut PgQueryDiagnostics,
        cgroup_pass: Option<&CgroupPass>,
    ) -> Result<PgCollectionOutcome> {
        let mut outcome = PgCollectionOutcome::default();
        let mut last_ts = None;
        let mut blocking = None;
        let result = pg
            .collect(
                due,
                &mut |observation| {
                    // Empty queries emit no batches. Only successful completion
                    // proves that blocking cleared; errors leave the cadence alone.
                    if let PgObservation::Query(query) = &observation
                        && query.query_name == "pg_locks"
                        && query.outcome == QueryOutcome::Success
                    {
                        blocking = Some(query.stats.rows > 0);
                    }
                    diagnostics.observe(observation);
                },
                |batch, settings| {
                    let Some(ts) = collection_timestamp_after(last_ts) else {
                        return Err(PgAppendError::Rejected);
                    };
                    last_ts = Some(ts);
                    let admitted = self.append_postgres(
                        &batch,
                        settings.as_deref().unwrap_or(&[]),
                        ts,
                        cgroup_pass,
                    )?;
                    outcome.written.extend(admitted.written);
                    outcome.appended = true;
                    outcome.opening_os_collected |= admitted.opening_os_collected;
                    Ok(admitted.write)
                },
            )
            .await;
        self.sched.finish_postgres(due, blocking, Instant::now());
        match result {
            Ok(()) | Err(PgAppendError::Rejected) => Ok(outcome),
            Err(PgAppendError::Fatal(error)) => Err(error),
        }
    }

    /// Retain one `PostgreSQL` batch through a pre-append close and encode it once
    /// more against the new segment dictionary before the stream may advance.
    pub(crate) fn append_postgres(
        &mut self,
        batch: &PgBatch,
        opening_settings: &[SettingsRow],
        ts: i64,
        cgroup_pass: Option<&CgroupPass>,
    ) -> Result<PgPendingOutcome, PgAppendError> {
        let mut written = Vec::new();
        let mut encode_elapsed = Duration::ZERO;
        let mut append_elapsed = Duration::ZERO;
        for attempt in 0..2 {
            let opening_due = self.segment.is_empty().then(|| {
                self.sched
                    .recollection_due(&DueSet::default(), Instant::now())
            });
            let includes_settings = matches!(batch, PgBatch::Settings(_))
                || (!self.segment.pg_settings_present && !opening_settings.is_empty());
            let buffered = self
                .buffer_postgres(
                    batch,
                    opening_settings,
                    opening_due.as_ref(),
                    ts,
                    cgroup_pass,
                )
                .map_err(|()| {
                    PgAppendError::Fatal(anyhow::anyhow!(
                        "buffer the PostgreSQL batch after updating segment state"
                    ))
                })?;
            let encode_started = Instant::now();
            let flushed = match encode_window(buffered.buffers, &self.segment.interner) {
                Ok(flushed) => flushed,
                Err(err) => {
                    log_event(
                        LogLevel::Error,
                        "window_encode_failure",
                        &[field("error", format!("{err:#}"))],
                    );
                    return Err(PgAppendError::Fatal(
                        err.context("encode the PostgreSQL batch"),
                    ));
                }
            };
            encode_elapsed = encode_elapsed.saturating_add(encode_started.elapsed());
            let encoded_bytes = u64::try_from(flushed.summary.part_bytes).unwrap_or(u64::MAX);
            let append_started = Instant::now();
            let finished = match append_window_and_maybe_close(
                self.journal,
                self.owner,
                self.config,
                self.segment,
                ts,
                false,
                &flushed,
            ) {
                Ok(finished) => finished,
                Err(failure) => return Err(pg_append_error(failure)),
            };
            append_elapsed = append_elapsed.saturating_add(append_started.elapsed());
            let retry = finished.iter().any(|(_, reason)| *reason == "journal-full");
            for (dest, reason) in finished {
                self.sched.mark_segment_opened();
                report_written(&dest, reason);
                written.push(dest);
            }
            if retry {
                if attempt != 0 {
                    return Err(PgAppendError::Fatal(anyhow::anyhow!(
                        "a fresh segment unexpectedly requested another pre-append close"
                    )));
                }
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
            let frame_bytes = encoded_bytes.saturating_add(
                u64::try_from(kronika_format::FRAME_HEADER_LEN).unwrap_or(u64::MAX),
            );
            return Ok(PgPendingOutcome {
                written,
                opening_os_collected: opening_due.is_some(),
                write: BatchWrite {
                    encode_elapsed,
                    append_elapsed,
                    encoded_bytes,
                    wal_bytes_appended: frame_bytes,
                },
            });
        }
        Err(PgAppendError::Fatal(anyhow::anyhow!(
            "a retained PostgreSQL batch exhausted its append attempts"
        )))
    }

    fn buffer_postgres(
        &mut self,
        batch: &PgBatch,
        opening_settings: &[SettingsRow],
        opening_due: Option<&DueSet>,
        ts: i64,
        cgroup_pass: Option<&CgroupPass>,
    ) -> Result<BufferedWindow, ()> {
        let mut buffers = SectionBuffers::new();
        let mut pending_users = Vec::new();
        let mut pending_cgroup_context = None;
        if self.segment.is_empty() {
            push_instance_metadata(
                &mut buffers,
                &mut self.segment.interner,
                self.in_container,
                self.config,
                ts,
            )
            .map_err(|err| {
                log_buffer_failure(&err);
            })?;
        }
        let settings = if self.segment.pg_settings_present {
            &[]
        } else {
            opening_settings
        };
        if let Some(due) = opening_due
            && let Some(process_io) = self.process_io.as_mut()
        {
            let fs = ProcFs::from_env();
            let mut os = collect_os_sources(
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
            );
            pending_cgroup_context = os.deduplicate_context(self.segment.cgroup_context.as_ref());
            pending_users.extend_from_slice(os.pending_users());
            push_os_sources(&mut buffers, &os).map_err(|err| {
                log_buffer_failure(&err);
            })?;
        }
        push_pg_batch(&mut buffers, &mut self.segment.interner, batch, settings).map_err(
            |err| {
                log_buffer_failure(&err);
            },
        )?;
        Ok(BufferedWindow {
            buffers,
            pending_users,
            pending_cgroup_context,
        })
    }
}

fn pg_append_error(failure: AppendWindowError) -> PgAppendError {
    let (close_failed, err) = failure.into_parts();
    log_event(
        LogLevel::Error,
        "window_append_failure",
        &[field("error", format!("{err:#}"))],
    );
    let context = if close_failed {
        "close the segment while appending a PostgreSQL batch"
    } else {
        "append the PostgreSQL batch to the journal"
    };
    PgAppendError::Fatal(err.context(context))
}
