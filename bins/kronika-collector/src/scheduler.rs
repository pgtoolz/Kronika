//! Per-source pacing for collection ticks.
//!
//! Each source has its own interval. A forced tick (`SIGUSR2`, and the first
//! timer tick after start) reads every paced source, so signal-driven collection
//! keeps its contract.
//! Positive intervals can pull the next timer wake forward; explicit zero
//! intervals run on every timer wake.

use std::time::{Duration, Instant};

/// One independently paced source group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::enum_variant_names,
    reason = "the names match the public OS section groups and their configuration"
)]
pub(crate) enum SourceKind {
    OsCore,
    OsMountTopo,
    OsProcesses,
    OsProcessStatus,
    OsCgroup,
    OsCgroupMapping,
    Logs,
    Pg,
    PgRelations,
}

/// All source kinds, in collection order.
pub(crate) const ALL_SOURCES: [SourceKind; 9] = [
    SourceKind::OsCore,
    SourceKind::OsMountTopo,
    SourceKind::OsProcesses,
    SourceKind::OsProcessStatus,
    SourceKind::OsCgroup,
    SourceKind::OsCgroupMapping,
    SourceKind::Logs,
    SourceKind::Pg,
    SourceKind::PgRelations,
];

/// Per-source intervals, in seconds.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::struct_field_names,
    reason = "the names match the documented OS interval configuration"
)]
pub(crate) struct Intervals {
    pub os_core: u64,
    pub os_mount_topo: u64,
    pub os_processes: u64,
    pub os_process_status: u64,
    pub os_cgroup: u64,
    pub os_cgroup_mapping: u64,
    pub logs: u64,
    pub pg: u64,
    pub pg_relations: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            os_core: 10,
            os_mount_topo: 60,
            os_processes: 5,
            os_process_status: 30,
            os_cgroup: 30,
            os_cgroup_mapping: 30,
            logs: 10,
            pg: 30,
            pg_relations: 300,
        }
    }
}

impl Intervals {
    const fn of(&self, kind: SourceKind) -> u64 {
        match kind {
            SourceKind::OsCore => self.os_core,
            SourceKind::OsMountTopo => self.os_mount_topo,
            SourceKind::OsProcesses => self.os_processes,
            SourceKind::OsProcessStatus => self.os_process_status,
            SourceKind::OsCgroup => self.os_cgroup,
            SourceKind::OsCgroupMapping => self.os_cgroup_mapping,
            SourceKind::Logs => self.logs,
            SourceKind::Pg => self.pg,
            SourceKind::PgRelations => self.pg_relations,
        }
    }
}

/// The sources one tick must read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DueSet {
    kinds: Vec<SourceKind>,
    forced: bool,
}

impl DueSet {
    /// Whether `kind` is due this tick.
    pub(crate) fn has(&self, kind: SourceKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// No source is due for this tick.
    pub(crate) const fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// Whether this tick was forced (SIGUSR2): paced reads outside the
    /// scheduler ignore their own deadline too.
    pub(crate) const fn forced(&self) -> bool {
        self.forced
    }

    /// A non-forced set for one incremental log batch.
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

    /// A set with every source due (forced tick).
    fn all() -> Self {
        Self {
            kinds: ALL_SOURCES.to_vec(),
            forced: true,
        }
    }

    /// A set with exactly the given sources due. For use in unit tests only.
    #[cfg(test)]
    #[allow(clippy::missing_const_for_fn, reason = "Vec disallows const context")]
    pub(crate) fn for_test(kinds: Vec<SourceKind>) -> Self {
        Self {
            kinds,
            forced: false,
        }
    }
}

/// The paced sources a fresh segment re-reads on its first tick.
const SEGMENT_OPEN_SOURCES: [SourceKind; 1] = [SourceKind::OsMountTopo];

/// Decides which sources each tick reads, one entry per source.
#[derive(Debug)]
pub(crate) struct Scheduler {
    intervals: Intervals,
    last_read: [Option<Instant>; ALL_SOURCES.len()],
}

impl Scheduler {
    pub(crate) const fn new(intervals: Intervals) -> Self {
        Self {
            intervals,
            last_read: [None; ALL_SOURCES.len()],
        }
    }

    /// A segment was just finished, so the next window must re-read the
    /// per-segment service sources.
    pub(crate) fn mark_segment_opened(&mut self) {
        for (slot, kind) in ALL_SOURCES.iter().enumerate() {
            if SEGMENT_OPEN_SOURCES.contains(kind) {
                self.last_read[slot] = None;
            }
        }
    }

    /// Build the due set for an immediate fresh-segment recollection.
    ///
    /// It preserves the triggering window's sources, adds the paced sources
    /// required at segment open, and records all of them as read at `now`.
    pub(crate) fn recollection_due(&mut self, due: &DueSet, now: Instant) -> DueSet {
        let kinds: Vec<SourceKind> = ALL_SOURCES
            .iter()
            .copied()
            .filter(|kind| due.has(*kind) || SEGMENT_OPEN_SOURCES.contains(kind))
            .collect();
        for (slot, kind) in ALL_SOURCES.iter().enumerate() {
            if kinds.contains(kind) {
                self.last_read[slot] = Some(now);
            }
        }
        DueSet {
            kinds,
            forced: due.forced,
        }
    }

    /// Time until the next positive source interval elapses.
    ///
    /// Sources with no previous read and explicit zero intervals are read on
    /// the next timer wake; they do not pull the wake forward by themselves.
    pub(crate) fn next_elapsed_due_in(&self, now: Instant) -> Option<Duration> {
        ALL_SOURCES
            .iter()
            .enumerate()
            .filter_map(|(slot, kind)| {
                let last = self.last_read[slot]?;
                let secs = self.intervals.of(*kind);
                if secs == 0 {
                    return None;
                }
                let interval = Duration::from_secs(secs);
                Some(interval.saturating_sub(now.saturating_duration_since(last)))
            })
            .min()
    }

    /// The due set for a tick at `now`.
    ///
    /// A source is due when it has never been read or its interval elapsed.
    /// `force` marks everything due — the SIGUSR2 contract. Due sources are
    /// immediately marked read: a failed read retries after its interval,
    /// not on the next tick.
    pub(crate) fn plan(&mut self, now: Instant, force: bool) -> DueSet {
        if force {
            self.last_read = [Some(now); ALL_SOURCES.len()];
            return DueSet::all();
        }
        let mut kinds = Vec::new();
        for (slot, kind) in ALL_SOURCES.iter().enumerate() {
            let secs = self.intervals.of(*kind);
            let interval = Duration::from_secs(secs);
            let is_due =
                self.last_read[slot].is_none_or(|last| now.duration_since(last) >= interval);
            if is_due {
                self.last_read[slot] = Some(now);
                kinds.push(*kind);
            }
        }
        DueSet {
            kinds,
            forced: false,
        }
    }
}

#[cfg(test)]
mod tests;
