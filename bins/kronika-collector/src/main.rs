//! Linux system collector daemon.
//!
//! Configuration is environment-only; the one required variable is
//! `KRONIKA_STORAGE_DIR`. The process snapshots the OS sources on their own
//! intervals, appends each synchronized window to `<storage>/active.wal`, and
//! publishes immutable `<storage>/YYYY/MM/DD/<segment-id>.zms` segments by size,
//! age, journal pressure, or `SIGUSR2`.
//!
//! `SIGTERM` and `SIGINT` stop the loop without discarding the journal.
//! Startup recovery writes valid frames left by the preceding process. A failed
//! source is logged and retried on its next interval; bad startup
//! configuration, journal-open failures, and any persistence failure that
//! poisons the journal terminate the process.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod buffering;
mod capacity;
mod config;
mod help;
mod log_sources;
mod logging;
mod os_sources;
mod pg_sources;
mod rotation;
mod scheduler;
mod segments;
mod service_sections;

use anyhow::{Context, Result};
use config::Config;
use kronika_layout::{DataRoot, LayoutLimits, TemporaryKind, WriterOwner};
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_source_os::{OsScope, ProcFs, detect_container};
use kronika_source_pg::query::BatchWrite;
use kronika_writer::{Journal, SectionBuffers};
use log_sources::{LogRows, LogSources, push_log_sources};
use logging::{LogLevel, field, log_event};
use os_sources::{collect_os_sources, push_os_sources};
use pg_sources::telemetry::PgTelemetry;
use pg_sources::{PgBatch, PgSources, push_pg_batch, push_pg_settings};
use rotation::Rotation;
use scheduler::{DueSet, Scheduler, SourceKind};
use segments::{
    AppendWindowError, SegmentState, append_window_and_maybe_close, close_open_segment,
    encode_window, open_collector_journal,
};
use service_sections::{collect_instance, push_instance_metadata};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::signal::unix::{SignalKind, signal};

/// How long to sleep before the next wake, or `None` to wait on signals only.
fn timer_sleep_delay(
    now: Instant,
    tick_secs: u64,
    segment_max_age_secs: u64,
    sched: &Scheduler,
    segment: &SegmentState,
    rotation: Option<&Rotation>,
) -> Option<Duration> {
    let mut delay = (tick_secs != 0).then(|| Duration::from_secs(tick_secs));
    if let Some(delay) = delay.as_mut() {
        if let Some(next_due) = sched.next_elapsed_due_in(now) {
            *delay = (*delay).min(next_due);
        }
        if let Some(next_age) =
            segment.time_until_age(now, Duration::from_secs(segment_max_age_secs))
        {
            *delay = (*delay).min(next_age);
        }
    }
    // Rotation runs its own timer, so an otherwise signal-only loop still wakes
    // for the periodic size re-check.
    if let Some(rotation) = rotation {
        let next_rotation = rotation.time_until_tick(now);
        delay = Some(delay.map_or(next_rotation, |delay| delay.min(next_rotation)));
    }
    delay
}

/// Delete temporaries a previous process left behind.
///
/// A temporary is a segment that was never published, so nothing reads it and
/// nothing is lost by removing it.
fn remove_writer_temporaries(owner: &WriterOwner, limits: LayoutLimits) -> Result<()> {
    let snapshot = owner
        .root()
        .scan(limits)
        .context("scan for stale writer temporaries")?;
    for temporary in &snapshot.temporaries {
        if temporary.kind != TemporaryKind::Zms {
            continue;
        }
        if let Err(error) = owner.remove_temporary(temporary) {
            log_event(
                LogLevel::Warn,
                "writer_temporary_remove_failed",
                &[field("error", format!("{error}"))],
            );
        }
    }
    Ok(())
}

fn prepare_collector_storage(
    owner: &WriterOwner,
    limits: LayoutLimits,
    journal_max_bytes: u64,
) -> Result<(Journal, Option<PathBuf>)> {
    remove_writer_temporaries(owner, limits)?;
    open_collector_journal(owner, journal_max_bytes)
}

/// Take exclusive ownership of the data root and recover what the previous
/// process left behind.
fn start_up(config: &Config) -> Result<(WriterOwner, Journal)> {
    std::fs::create_dir_all(&config.storage_dir).context("create the storage directory")?;
    let data_root = DataRoot::open(&config.storage_dir).context("open the data root")?;
    let writer_owner = data_root
        .acquire_writer(LayoutLimits::default())
        .context("acquire exclusive writer ownership")?;
    // The journal lives next to finished segments so windows survive restarts.
    let (journal, recovered) = prepare_collector_storage(
        &writer_owner,
        LayoutLimits::default(),
        config.journal_max_bytes,
    )?;
    if let Some(dest) = recovered {
        announce(&format!("wrote {} reason=recovered", dest.display()));
    }
    Ok((writer_owner, journal))
}

fn initialize_collector(config: &Config) -> Result<(WriterOwner, Journal, LogSources, PgSources)> {
    let logs = LogSources::open(config).context("open the configured log files")?;
    let pg = PgSources::open(config).context("open the configured PostgreSQL metrics")?;
    let (writer_owner, journal) = start_up(config)?;
    Ok((writer_owner, journal, logs, pg))
}

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    if let Some(argument) = arguments.next() {
        anyhow::ensure!(
            arguments.next().is_none(),
            "unexpected arguments; use kronika-collector --help"
        );
        if argument == "--help" || argument == "-h" {
            print!("{}", help::HELP);
            return Ok(());
        }
        if argument == "--version" {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if capacity::is_helper_invocation() {
            return capacity::run_helper();
        }
        anyhow::bail!(
            "unexpected argument {}; use kronika-collector --help",
            argument.display()
        );
    }
    run_collector()
}

#[tokio::main]
#[allow(
    clippy::too_many_lines,
    reason = "the top-level signal and persistence loop must share shutdown telemetry"
)]
async fn run_collector() -> Result<()> {
    let config = Config::from_env()?;
    let (writer_owner, mut journal, mut logs, mut pg) = initialize_collector(&config)?;
    let in_container = detect_container(&ProcFs::from_env());
    let mut pg_telemetry = PgTelemetry::new(Instant::now());

    let mut sigusr2 = signal(SignalKind::user_defined2()).context("install the SIGUSR2 handler")?;
    let mut sigterm = signal(SignalKind::terminate()).context("install the SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install the SIGINT handler")?;
    let mut sched = Scheduler::new(config.intervals);
    let mut process_io = ProcessIoCredentials::new();
    let mut segment = SegmentState::default();
    let mut rotation = Rotation::new(
        config.retention,
        &writer_owner,
        LayoutLimits::default(),
        Instant::now(),
    )?;
    // With the timer disabled collection is signal-driven only.
    let mut first_timer_tick = config.tick_secs > 0;

    announce("ready");

    let result: Result<()> = async {
        loop {
            pg_telemetry.maybe_emit(Instant::now());
            let sleep = if first_timer_tick {
                first_timer_tick = false;
                Some(Duration::ZERO)
            } else {
                timer_sleep_delay(
                    Instant::now(),
                    config.tick_secs,
                    config.segment_max_age_secs,
                    &sched,
                    &segment,
                    rotation.as_ref(),
                )
            };
            let forced = tokio::select! {
                Some(()) = sigusr2.recv() => true,
                () = async {
                    match sleep {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    // With the collection timer disabled the only timed wake is
                    // rotation's; collection stays strictly signal-driven.
                    if config.tick_secs == 0 {
                        run_rotation(&mut rotation, &writer_owner, &journal, &[]);
                        continue;
                    }
                    false
                }
                _ = sigterm.recv() => break,
                _ = sigint.recv() => break,
            };
            let due = sched.plan(Instant::now(), forced);
            let mut written_this_tick: Vec<PathBuf> = Vec::new();
            // The age valve runs before collection: a tick whose sources fail or
            // return no rows must still close an expired segment.
            let age = Duration::from_secs(config.segment_max_age_secs);
            if segment.age_expired(Instant::now(), age) {
                let dest = close_open_segment(&mut journal, &writer_owner, &mut segment, "age")?;
                sched.mark_segment_opened();
                announce(&format!("wrote {} reason=age", dest.display()));
                written_this_tick.push(dest);
                stop_if_persistence_unhealthy(&journal)?;
            }
            if due.is_empty() {
                run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
                continue;
            }
            logs.rescan(&mut |observation| pg_telemetry.observe(observation))
                .await;
            // PostgreSQL batches reach the WAL before the query stream fetches
            // another batch. They never ride on the incremental log windows.
            let shutdown = async {
                tokio::select! {
                    _ = sigterm.recv() => (),
                    _ = sigint.recv() => (),
                }
            };
            let Some(pg_outcome) = complete_or_shutdown(
                Box::pin(run_pg_collection_cycle(
                    &mut pg,
                    &mut journal,
                    &writer_owner,
                    &config,
                    in_container,
                    &due,
                    &mut process_io,
                    &mut segment,
                    &mut sched,
                    &mut pg_telemetry,
                )),
                shutdown,
            )
            .await
            else {
                pg.close_connections();
                break;
            };
            let pg_outcome = pg_outcome?;
            written_this_tick.extend(pg_outcome.written);
            let opening_settings = pg.last_settings();
            let collection_due = if pg_outcome.opening_os_collected && !segment.is_empty() {
                due.without(SourceKind::OsMountTopo)
            } else {
                due.clone()
            };
            written_this_tick.extend(run_collection_cycle(
                &mut journal,
                &writer_owner,
                &config,
                in_container,
                &collection_due,
                opening_settings.as_deref().unwrap_or(&[]),
                &mut process_io,
                &mut segment,
                &mut sched,
                &mut logs,
                pg_outcome.appended,
            )?);
            stop_if_persistence_unhealthy(&journal)?;
            run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
        }
        Ok(())
    }
    .await;
    pg_telemetry.shutdown(Instant::now());
    result
}

async fn complete_or_shutdown<T>(
    work: impl Future<Output = T>,
    shutdown: impl Future<Output = ()>,
) -> Option<T> {
    tokio::pin!(work);
    tokio::pin!(shutdown);
    tokio::select! {
        biased;
        () = &mut shutdown => None,
        output = &mut work => Some(output),
    }
}

#[derive(Default)]
struct PgCollectionOutcome {
    written: Vec<PathBuf>,
    appended: bool,
    opening_os_collected: bool,
}

#[derive(Debug)]
enum PgAppendError {
    Rejected,
    Fatal(anyhow::Error),
}

/// Stream due `PostgreSQL` sources into independently admitted WAL parts.
#[allow(
    clippy::too_many_arguments,
    reason = "the PostgreSQL stream needs the same durable segment state as ordinary collection"
)]
async fn run_pg_collection_cycle(
    pg: &mut PgSources,
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    in_container: bool,
    due: &DueSet,
    process_io: &mut ProcessIoCredentials,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
    telemetry: &mut PgTelemetry,
) -> Result<PgCollectionOutcome> {
    let mut outcome = PgCollectionOutcome::default();
    let mut last_ts = None;
    let result = pg
        .collect(
            due,
            &mut |observation| telemetry.observe(observation),
            |batch, settings| {
                let Some(ts) = collection_timestamp_after(last_ts) else {
                    return Err(PgAppendError::Rejected);
                };
                last_ts = Some(ts);
                let admitted = append_pending_pg_batch(
                    journal,
                    writer_owner,
                    config,
                    in_container,
                    &batch,
                    settings.as_deref().unwrap_or(&[]),
                    ts,
                    process_io,
                    segment,
                    sched,
                )?;
                outcome.written.extend(admitted.written);
                outcome.appended = true;
                outcome.opening_os_collected |= admitted.opening_os_collected;
                Ok(admitted.write)
            },
        )
        .await;
    match result {
        Ok(()) | Err(PgAppendError::Rejected) => Ok(outcome),
        Err(PgAppendError::Fatal(error)) => Err(error),
    }
}

struct PgPendingOutcome {
    written: Vec<PathBuf>,
    write: BatchWrite,
    opening_os_collected: bool,
}

struct BufferedWindow {
    buffers: SectionBuffers,
    pending_users: Vec<(u8, u32)>,
}

/// Retain one `PostgreSQL` batch through a pre-append close and encode it once
/// more against the new segment dictionary before the stream may advance.
#[allow(
    clippy::too_many_arguments,
    reason = "a retained batch needs the journal, owner, limits, timestamp, and segment state"
)]
fn append_pending_pg_batch(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    in_container: bool,
    batch: &PgBatch,
    opening_settings: &[kronika_source_pg::settings::SettingsRow],
    ts: i64,
    process_io: &mut ProcessIoCredentials,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
) -> std::result::Result<PgPendingOutcome, PgAppendError> {
    let mut written = Vec::new();
    let mut encode_elapsed = Duration::ZERO;
    let mut append_elapsed = Duration::ZERO;
    for attempt in 0..2 {
        let opening_due = segment
            .is_empty()
            .then(|| sched.recollection_due(&DueSet::default(), Instant::now()));
        let includes_settings = matches!(batch, PgBatch::Settings(_))
            || (segment.needs_pg_settings() && !opening_settings.is_empty());
        let buffered = buffer_pg_batch(
            segment,
            batch,
            opening_settings,
            opening_due.as_ref(),
            config,
            in_container,
            ts,
            process_io,
        )
        .map_err(|()| {
            PgAppendError::Fatal(anyhow::anyhow!(
                "buffer the PostgreSQL batch after updating segment state"
            ))
        })?;
        let encode_started = Instant::now();
        let flushed = match encode_window(buffered.buffers, segment.interner()) {
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
            journal,
            writer_owner,
            config,
            segment,
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
            sched.mark_segment_opened();
            announce(&format!("wrote {} reason={reason}", dest.display()));
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
        if includes_settings && !segment.is_empty() {
            segment.mark_pg_settings_present();
        }
        if !segment.is_empty() {
            segment.mark_users_recorded(&buffered.pending_users);
        }
        let frame_bytes = encoded_bytes
            .saturating_add(u64::try_from(kronika_format::FRAME_HEADER_LEN).unwrap_or(u64::MAX));
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

#[allow(
    clippy::too_many_arguments,
    reason = "the retained batch shares collector-lifetime process access state"
)]
fn buffer_pg_batch(
    segment: &mut SegmentState,
    batch: &PgBatch,
    opening_settings: &[kronika_source_pg::settings::SettingsRow],
    opening_due: Option<&DueSet>,
    config: &Config,
    in_container: bool,
    ts: i64,
    process_io: &mut ProcessIoCredentials,
) -> std::result::Result<BufferedWindow, ()> {
    let fs = ProcFs::from_env();
    let mut buffers = SectionBuffers::new();
    let mut pending_users = Vec::new();
    if segment.is_empty() {
        let facts = collect_instance().map_err(|err| {
            log_buffer_failure(&err);
        })?;
        push_instance_metadata(
            &mut buffers,
            segment.interner_mut(),
            &facts,
            in_container,
            config,
            ts,
        )
        .map_err(|err| {
            log_buffer_failure(&err);
        })?;
    }
    let settings = if segment.needs_pg_settings() {
        opening_settings
    } else {
        &[]
    };
    if let Some(due) = opening_due {
        let os = {
            let (interner, users) = segment.os_state_mut();
            collect_os_sources(
                &fs,
                process_io,
                interner,
                users,
                OsScope::Host.as_u8(),
                ts,
                in_container,
                due,
            )
        };
        pending_users.extend_from_slice(os.pending_users());
        push_os_sources(&mut buffers, &os).map_err(|err| {
            log_buffer_failure(&err);
        })?;
    }
    push_pg_batch(&mut buffers, segment.interner_mut(), batch, settings).map_err(|err| {
        log_buffer_failure(&err);
    })?;
    Ok(BufferedWindow {
        buffers,
        pending_users,
    })
}

/// One collection cycle: read the due sources, append bounded windows, and
/// write when the segment is full. Log input is acknowledged only after its
/// exact window reaches the WAL.
#[allow(
    clippy::too_many_arguments,
    reason = "one cycle needs the journal, the owner, the config, the due set, and the state each source keeps between ticks"
)]
fn run_collection_cycle(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    in_container: bool,
    due: &DueSet,
    opening_settings: &[kronika_source_pg::settings::SettingsRow],
    process_io: &mut ProcessIoCredentials,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
    logs: &mut LogSources,
    already_appended: bool,
) -> Result<Vec<PathBuf>> {
    let Some(parse_now) = collection_timestamp() else {
        return Ok(Vec::new());
    };
    let mut first_window = true;
    let mut last_ts = None;
    let mut written = Vec::new();
    let mut appended = already_appended;
    let completed = logs.collect(due, parse_now, |rows| {
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
        let outcome = append_pending_window(
            journal,
            writer_owner,
            config,
            in_container,
            &batch_due,
            rows,
            opening_settings,
            ts,
            process_io,
            segment,
            sched,
        )?;
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
        let outcome = append_pending_window(
            journal,
            writer_owner,
            config,
            in_container,
            due,
            &LogRows::default(),
            opening_settings,
            ts,
            process_io,
            segment,
            sched,
        )?;
        written.extend(outcome.written);
        appended |= outcome.appended;
    }

    // A forced cycle closes once, after all of its incremental log batches.
    if completed && appended && due.forced() && !segment.is_empty() {
        let dest = close_open_segment(journal, writer_owner, segment, "forced")?;
        sched.mark_segment_opened();
        announce(&format!("wrote {} reason=forced", dest.display()));
        written.push(dest);
    }
    Ok(written)
}

#[derive(Default)]
struct PendingWindowOutcome {
    written: Vec<PathBuf>,
    accepted: bool,
    appended: bool,
}

#[allow(
    clippy::too_many_arguments,
    reason = "a retained window needs the journal, owner, limits, due sources, timestamp, and segment state"
)]
fn append_pending_window(
    journal: &mut Journal,
    writer_owner: &WriterOwner,
    config: &Config,
    in_container: bool,
    due: &DueSet,
    log_rows: &LogRows,
    opening_settings: &[kronika_source_pg::settings::SettingsRow],
    ts: i64,
    process_io: &mut ProcessIoCredentials,
    segment: &mut SegmentState,
    sched: &mut Scheduler,
) -> Result<PendingWindowOutcome> {
    let mut outcome = PendingWindowOutcome::default();
    let mut attempt_due = due_for_window(segment, due, sched);
    for attempt in 0..2 {
        let fresh_segment = segment.is_empty();
        let includes_settings = segment.needs_pg_settings() && !opening_settings.is_empty();
        let buffered = match buffer_window(
            segment,
            &attempt_due,
            log_rows,
            opening_settings,
            config,
            in_container,
            ts,
            process_io,
        ) {
            Ok(Some(buffered)) => buffered,
            Ok(None) => {
                outcome.accepted = true;
                return Ok(outcome);
            }
            Err(BufferFailure) => {
                if fresh_segment {
                    anyhow::bail!("buffer the collection window after updating segment state");
                }
                return Ok(outcome);
            }
        };
        let flushed = match encode_window(buffered.buffers, segment.interner()) {
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
            journal,
            writer_owner,
            config,
            segment,
            ts,
            false,
            &flushed,
        ) {
            Ok(finished) => {
                let retry = finished.iter().any(|(_, reason)| *reason == "journal-full");
                for (dest, reason) in finished {
                    sched.mark_segment_opened();
                    announce(&format!("wrote {} reason={reason}", dest.display()));
                    outcome.written.push(dest);
                }
                if retry {
                    anyhow::ensure!(
                        attempt == 0,
                        "a fresh segment unexpectedly requested another pre-append close"
                    );
                    // Rebuild from owned logical rows. Section buffers and
                    // dictionary ids belong to the segment that just closed.
                    attempt_due = sched.recollection_due(due, Instant::now());
                    continue;
                }
                if includes_settings && !segment.is_empty() {
                    segment.mark_pg_settings_present();
                }
                if !segment.is_empty() {
                    segment.mark_users_recorded(&buffered.pending_users);
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

fn due_for_window(segment: &SegmentState, due: &DueSet, sched: &mut Scheduler) -> DueSet {
    if segment.is_empty() {
        sched.recollection_due(due, Instant::now())
    } else {
        due.clone()
    }
}

#[derive(Debug)]
struct BufferFailure;

/// Read the selected OS sources and add the retained log rows to one window.
#[allow(
    clippy::too_many_arguments,
    reason = "one buffered window shares collector-lifetime process access state"
)]
fn buffer_window(
    segment: &mut SegmentState,
    due: &DueSet,
    log_rows: &LogRows,
    opening_settings: &[kronika_source_pg::settings::SettingsRow],
    config: &Config,
    in_container: bool,
    ts: i64,
    process_io: &mut ProcessIoCredentials,
) -> std::result::Result<Option<BufferedWindow>, BufferFailure> {
    let fs = ProcFs::from_env();
    let mut buffers = SectionBuffers::new();
    if segment.is_empty() {
        let facts = collect_instance().map_err(|err| {
            log_buffer_failure(&err);
            BufferFailure
        })?;
        if let Err(err) = push_instance_metadata(
            &mut buffers,
            segment.interner_mut(),
            &facts,
            in_container,
            config,
            ts,
        ) {
            log_buffer_failure(&err);
            return Err(BufferFailure);
        }
    }

    let os = {
        let (interner, users) = segment.os_state_mut();
        collect_os_sources(
            &fs,
            process_io,
            interner,
            users,
            OsScope::Host.as_u8(),
            ts,
            in_container,
            due,
        )
    };
    let settings = if segment.needs_pg_settings() {
        opening_settings
    } else {
        &[]
    };
    if let Err(err) = push_pg_settings(&mut buffers, segment.interner_mut(), settings)
        .and_then(|()| push_os_sources(&mut buffers, &os))
        .and_then(|()| push_log_sources(&mut buffers, segment.interner_mut(), log_rows))
    {
        log_buffer_failure(&err);
        return Err(BufferFailure);
    }
    if buffers.is_empty() {
        return Ok(None);
    }
    Ok(Some(BufferedWindow {
        buffers,
        pending_users: os.pending_users().to_vec(),
    }))
}

fn log_buffer_failure(err: &anyhow::Error) {
    log_event(
        LogLevel::Error,
        "window_buffer_failure",
        &[field("error", format!("{err:#}"))],
    );
}

fn collection_timestamp() -> Option<i64> {
    match unix_now_us() {
        Ok(ts) => Some(ts),
        Err(err) => {
            log_event(
                LogLevel::Error,
                "collection_failed",
                &[field("reason", "clock"), field("error", format!("{err:#}"))],
            );
            None
        }
    }
}

fn collection_timestamp_after(previous: Option<i64>) -> Option<i64> {
    let now = collection_timestamp()?;
    previous.map_or(Some(now), |previous| {
        previous.checked_add(1).map(|next| now.max(next))
    })
}

fn unix_now_us() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?;
    i64::try_from(duration.as_micros()).context("system time exceeds i64 microseconds")
}

/// Feeds the tick's publications to rotation and lets it enforce the target.
///
/// A no-op when rotation is disabled. Publications grow the incremental size
/// counter; the enforcement itself is scan-free unless the tree is over target.
fn run_rotation(
    rotation: &mut Option<Rotation>,
    writer_owner: &WriterOwner,
    journal: &Journal,
    finished: &[PathBuf],
) {
    let Some(rotation) = rotation.as_mut() else {
        return;
    };
    for dest in finished {
        match std::fs::metadata(dest) {
            Ok(metadata) => rotation.record_publication(metadata.len()),
            // An uncounted publication under-counts the tree until the next
            // enforcement scan re-seeds the counter.
            Err(err) => log_event(
                LogLevel::Warn,
                "rotation_publication_stat_failure",
                &[
                    field("path", dest.display().to_string()),
                    field("error", format!("{err:#}")),
                ],
            ),
        }
    }
    let journal_bytes = u64::try_from(journal.bytes()).unwrap_or(u64::MAX);
    rotation.maybe_enforce(
        writer_owner,
        journal_bytes,
        !finished.is_empty(),
        Instant::now(),
    );
}

fn stop_if_persistence_unhealthy(journal: &Journal) -> Result<()> {
    if journal.is_poisoned() {
        anyhow::bail!(
            "active.wal entered an indeterminate persistence state; stop and recover it on restart"
        );
    }
    Ok(())
}

fn announce(line: &str) {
    let mut stdout = std::io::stdout().lock();
    drop(writeln!(stdout, "{line}").and_then(|()| stdout.flush()));
}

#[cfg(test)]
mod tests;
