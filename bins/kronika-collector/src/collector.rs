//! Collector lifecycle: initialize, wait for a tick, collect, rotate, shut down.

mod postgres;
mod startup;
mod window;

use anyhow::{Context, Result};
use kronika_layout::{LayoutLimits, WriterOwner};
use kronika_source_os::detect_container_with_root_override;
use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_writer::Journal;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::signal::unix::{SignalKind, signal};

use crate::cgroup_discovery;
use crate::clock::unix_now_us;
use crate::config::Config;
use crate::logging::{LogLevel, field, log_event};
use crate::pg_sources::query_diagnostics::PgQueryDiagnostics;
use crate::rotation::Rotation;
use crate::scheduler::{Scheduler, SourceKind};
use crate::segments::{SegmentState, close_open_segment, report_written};

pub(crate) use startup::initialize_collector;

/// State shared by `PostgreSQL` batches and OS/log windows during one tick.
/// The writer borrows it so stream callbacks use the same journal and dictionary.
pub(crate) struct WindowWriter<'a> {
    pub(crate) journal: &'a mut Journal,
    pub(crate) owner: &'a WriterOwner,
    pub(crate) config: &'a Config,
    pub(crate) in_container: bool,
    pub(crate) process_io: &'a mut Option<ProcessIoCredentials>,
    pub(crate) segment: &'a mut SegmentState,
    pub(crate) sched: &'a mut Scheduler,
}

#[allow(
    clippy::too_many_lines,
    reason = "the top-level signal and persistence loop must share shutdown diagnostics"
)]
pub(crate) async fn run() -> Result<()> {
    let config = crate::config::get();
    let (writer_owner, mut journal, mut logs, mut pg) = initialize_collector(config)?;
    let prometheus = if crate::prometheus::enabled(config) {
        let exporter = crate::prometheus::start(config)
            .await
            .context("start the Prometheus exporter")?;
        let server = std::sync::Arc::clone(&exporter).serve();
        tokio::spawn(server);
        Some(exporter)
    } else {
        None
    };
    let mut last_discovery_generation = None;
    let fs = config.proc_fs();
    let sys = config.sys_fs();
    let in_container = config.mode.collect_os()
        && detect_container_with_root_override(&fs, config.proc_root.is_some());
    let collect_cgroups = cgroup_discovery::enabled(&fs, config.mode, in_container);
    let mut query_diagnostics = PgQueryDiagnostics::new(Instant::now());

    let mut sigusr2 = signal(SignalKind::user_defined2()).context("install the SIGUSR2 handler")?;
    let mut sigterm = signal(SignalKind::terminate()).context("install the SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("install the SIGINT handler")?;
    let mut sched = Scheduler::new(config.intervals, config.mode, collect_cgroups);
    sched.probe_psi(in_container, || {
        std::fs::File::open(fs.path("pressure/cpu")?)?.read(&mut [0])
    });
    let mut process_io = config.mode.collect_os().then(ProcessIoCredentials::new);
    let seal_seed = writer_owner
        .load_or_create_seal_seed()
        .context("load the storage seal seed")?;
    let mut segment = SegmentState::with_seal_seed(seal_seed);
    let mut rotation = Rotation::new(
        config.retention,
        &writer_owner,
        LayoutLimits::default(),
        Instant::now(),
    )?;
    // With the timer disabled collection is signal-driven only.
    let mut first_timer_tick = config.tick_secs > 0;

    {
        // Readiness must reach a pipe immediately; output errors do not stop collection.
        let mut stdout = std::io::stdout().lock();
        drop(writeln!(stdout, "ready").and_then(|()| stdout.flush()));
    }

    let result: Result<()> = async {
        loop {
            query_diagnostics.maybe_emit(Instant::now());
            let sleep = if first_timer_tick {
                first_timer_tick = false;
                Some(Duration::ZERO)
            } else {
                timer_sleep_delay(
                    Instant::now(),
                    config.tick_secs,
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
            if segment.age_expired(Instant::now()) {
                let dest = close_open_segment(&mut journal, &writer_owner, &mut segment, "age")?;
                sched.mark_segment_opened();
                report_written(&dest, "age");
                written_this_tick.push(dest);
                stop_if_persistence_unhealthy(&journal)?;
            }
            if due.is_empty() {
                run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
                continue;
            }
            let mut cgroup_pass = if config.mode.collect_os() && due.has(SourceKind::OsCgroup) {
                let ts = unix_now_us()?;
                Some(cgroup_discovery::run(
                    &fs,
                    &sys,
                    config,
                    in_container,
                    &mut journal,
                    &writer_owner,
                    &mut segment,
                    &mut sched,
                    ts,
                    pg.last_settings().as_deref().unwrap_or(&[]),
                )?)
            } else {
                None
            };
            if let Some(pass) = cgroup_pass.as_mut() {
                written_this_tick.append(&mut pass.written);
            }
            logs.rescan(&mut |observation| query_diagnostics.observe(observation))
                .await;
            // PostgreSQL batches reach the WAL before the query stream fetches
            // another batch. They never ride on the incremental log windows.
            let shutdown = async {
                tokio::select! {
                    _ = sigterm.recv() => (),
                    _ = sigint.recv() => (),
                }
            };
            let mut writer = WindowWriter {
                journal: &mut journal,
                owner: &writer_owner,
                config,
                in_container,
                process_io: &mut process_io,
                segment: &mut segment,
                sched: &mut sched,
            };
            let Some(pg_outcome) = complete_or_shutdown(
                Box::pin(writer.collect_postgres(
                    &mut pg,
                    &due,
                    &mut query_diagnostics,
                    cgroup_pass.as_ref(),
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
            // The exporter rides on the collector tick and reuses the
            // collector's database discovery (model: pgwatch v5, scrape from
            // cache). EXE-8 disables lift on a new discovery cycle.
            if let Some(exporter) = &prometheus {
                let generation = pg.discovery_generation();
                let refreshed = last_discovery_generation.is_some_and(|last| last != generation);
                last_discovery_generation = Some(generation);
                exporter
                    .run_pass(&pg.discovered_database_names(), refreshed)
                    .await;
            }
            let opening_settings = pg.last_settings();
            let collection_due = if pg_outcome.opening_os_collected && !writer.segment.is_empty() {
                due.without(SourceKind::OsMountTopo)
            } else {
                due.clone()
            };
            written_this_tick.extend(writer.collect_os_and_logs(
                &collection_due,
                opening_settings.as_deref().unwrap_or(&[]),
                &mut logs,
                pg_outcome.appended || cgroup_pass.as_ref().is_some_and(|pass| pass.appended),
                cgroup_pass.as_ref(),
            )?);
            stop_if_persistence_unhealthy(&journal)?;
            run_rotation(&mut rotation, &writer_owner, &journal, &written_this_tick);
        }
        Ok(())
    }
    .await;
    query_diagnostics.shutdown(Instant::now());
    result
}

pub(crate) async fn complete_or_shutdown<T>(
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

/// How long to sleep before the next wake, or `None` to wait on signals only.
pub(crate) fn timer_sleep_delay(
    now: Instant,
    tick_secs: u64,
    sched: &Scheduler,
    segment: &SegmentState,
    rotation: Option<&Rotation>,
) -> Option<Duration> {
    let mut delay = (tick_secs != 0).then(|| Duration::from_secs(tick_secs));
    if let Some(delay) = delay.as_mut() {
        if let Some(next_due) = sched.next_elapsed_due_in(now) {
            *delay = (*delay).min(next_due);
        }
        if let Some(next_age) = segment.time_until_age(now) {
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

/// Feeds the tick's publications to rotation and lets it enforce the target.
///
/// A no-op when rotation is disabled. Publications grow the incremental size
/// counter; rotation scans only when over target or the fixed-budget recount is due.
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
