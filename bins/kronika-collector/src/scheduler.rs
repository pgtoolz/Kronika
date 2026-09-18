//! Select source groups whose collection intervals have elapsed.
//!
//! The first tick reads every enabled group. SIGUSR2 bypasses ordinary intervals,
//! but statements and plans always wait at least five minutes between reads.
//! Activity accelerates while blocked sessions exist. Ordinary zero intervals
//! mean every tick, without scheduling extra wakes.

mod sources;

use crate::config::CollectorMode;
use sources::ALL_SOURCES;
pub(crate) use sources::{Intervals, MIN_PG_STATEMENTS_INTERVAL_SECS, SourceKind};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ScheduledSource {
    kind: SourceKind,
    interval: Duration,
    last_read: Option<Instant>,
}

/// Holds the interval and last scheduled read for each enabled group.
#[derive(Debug)]
pub(crate) struct Scheduler {
    sources: Vec<ScheduledSource>,
    collect_psi: bool,
    /// Restore this cadence after a successful activity read finds no blockers.
    pg_activity_interval: Duration,
    /// Apply this cadence during lock waits, capped by the ordinary interval.
    pg_activity_blocked_interval: Duration,
}

impl Scheduler {
    pub(crate) fn new(intervals: Intervals, mode: CollectorMode, collect_cgroups: bool) -> Self {
        let sources = ALL_SOURCES
            .into_iter()
            .filter(|kind| {
                mode.collect_os()
                    || matches!(
                        kind,
                        SourceKind::Logs
                            | SourceKind::PgActivity
                            | SourceKind::PgInstance
                            | SourceKind::PgTablesAndIndexes
                            | SourceKind::PgStatementsAndPlans
                    )
            })
            .filter(|kind| {
                collect_cgroups
                    || !matches!(kind, SourceKind::OsCgroup | SourceKind::OsCgroupMapping)
            })
            .map(|kind| {
                let seconds = intervals.of(kind);
                let seconds = if kind == SourceKind::PgStatementsAndPlans {
                    seconds.max(MIN_PG_STATEMENTS_INTERVAL_SECS)
                } else {
                    seconds
                };
                ScheduledSource {
                    kind,
                    interval: Duration::from_secs(seconds),
                    last_read: None,
                }
            })
            .collect();
        Self {
            sources,
            collect_psi: mode.collect_os(),
            pg_activity_interval: Duration::from_secs(intervals.pg_activity),
            pg_activity_blocked_interval: Duration::from_secs(intervals.pg_activity_blocked),
        }
    }

    pub(crate) fn probe_psi(
        &mut self,
        in_container: bool,
        read: impl FnOnce() -> std::io::Result<usize>,
    ) {
        if in_container && !self.collects_cgroups() {
            self.collect_psi = false;
        }
        if !self.collect_psi {
            return;
        }
        if let Err(error) = read()
            && error.raw_os_error() == Some(rustix::io::Errno::OPNOTSUPP.raw_os_error())
        {
            self.collect_psi = false;
            crate::logging::log_event(
                crate::logging::LogLevel::Warn,
                "psi_disabled",
                &[crate::logging::field(
                    "reason",
                    "kernel PSI unsupported. Collection disabled until collector restart",
                )],
            );
        }
    }

    pub(crate) const fn collects_psi(&self) -> bool {
        self.collect_psi
    }

    pub(crate) fn collects_cgroups(&self) -> bool {
        self.sources
            .iter()
            .any(|source| source.kind == SourceKind::OsCgroup)
    }

    /// Start the statement cooldown after the whole `PostgreSQL` pass, including
    /// failures: a late extension query must not shorten the next five-minute gap.
    /// Unknown blocking state keeps the last activity cadence until a clear read.
    pub(crate) fn finish_postgres(&mut self, due: &DueSet, blocking: Option<bool>, now: Instant) {
        for source in &mut self.sources {
            match source.kind {
                SourceKind::PgStatementsAndPlans if due.has(source.kind) => {
                    source.last_read = Some(now);
                }
                SourceKind::PgActivity => {
                    if let Some(blocking) = blocking {
                        source.interval = if blocking {
                            self.pg_activity_interval
                                .min(self.pg_activity_blocked_interval)
                        } else {
                            self.pg_activity_interval
                        };
                    }
                }
                _ => {}
            }
        }
    }

    /// Schedule unread or elapsed groups; force bypasses all but the statement interval.
    /// Mark them now, so a failed collection still waits its interval before retrying.
    pub(crate) fn plan(&mut self, now: Instant, force: bool) -> DueSet {
        let mut kinds = Vec::new();
        for source in &mut self.sources {
            if (force && source.kind != SourceKind::PgStatementsAndPlans)
                || source
                    .last_read
                    .is_none_or(|last| now.duration_since(last) >= source.interval)
            {
                source.last_read = Some(now);
                kinds.push(source.kind);
            }
        }
        DueSet {
            kinds,
            forced: force,
        }
    }

    /// Time until the next positive interval elapses. Unread groups and zero
    /// intervals wait for the next regular tick instead of advancing the wake.
    pub(crate) fn next_elapsed_due_in(&self, now: Instant) -> Option<Duration> {
        self.sources
            .iter()
            .filter(|source| !source.interval.is_zero())
            .filter_map(|source| {
                let last = source.last_read?;
                Some(
                    source
                        .interval
                        .saturating_sub(now.saturating_duration_since(last)),
                )
            })
            .min()
    }

    /// The next OS window needs fresh mounts, topology and `CPUFreq` policies in
    /// the new segment, even if their normal interval has not elapsed.
    pub(crate) fn mark_segment_opened(&mut self) {
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.kind == SourceKind::OsMountTopo)
        {
            source.last_read = None;
        }
    }

    /// Rebuild a window after opening a segment: repeat its enabled groups,
    /// include mount/topology metadata and start their intervals at `now`.
    /// OS recollection does not run statements again or move their cooldown.
    pub(crate) fn recollection_due(&mut self, due: &DueSet, now: Instant) -> DueSet {
        let mut kinds = Vec::new();
        for source in &mut self.sources {
            if due.has(source.kind) || source.kind == SourceKind::OsMountTopo {
                if source.kind != SourceKind::PgStatementsAndPlans {
                    source.last_read = Some(now);
                }
                kinds.push(source.kind);
            }
        }
        DueSet {
            kinds,
            forced: due.forced,
        }
    }
}

/// Source groups selected for one collection window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DueSet {
    kinds: Vec<SourceKind>,
    forced: bool,
}

impl DueSet {
    pub(crate) fn has(&self, kind: SourceKind) -> bool {
        self.kinds.contains(&kind)
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// SIGUSR2 also bypasses internal discovery deadlines in the collectors.
    pub(crate) const fn forced(&self) -> bool {
        self.forced
    }

    /// A non-forced window for the next incremental log batch.
    pub(crate) fn logs() -> Self {
        Self {
            kinds: vec![SourceKind::Logs],
            forced: false,
        }
    }

    pub(crate) fn without(&self, excluded: SourceKind) -> Self {
        Self {
            kinds: self
                .kinds
                .iter()
                .copied()
                .filter(|kind| *kind != excluded)
                .collect(),
            forced: self.forced,
        }
    }
}

#[cfg(test)]
#[path = "tests/scheduler.rs"]
mod tests;
