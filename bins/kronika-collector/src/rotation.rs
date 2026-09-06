//! Storage rotation for the `KRONIKA_STORAGE_DIR` tree.
//!
//! Rotation keeps the whole data tree inside a fixed byte budget or a partition
//! used-fraction target by deleting the oldest replaceable data. It runs after
//! every ZMS publication and on a periodic tick, never deletes the active
//! journal or the newest finished segment, and emits a degradation event when
//! only that non-deletable minimum remains. The tree size is tracked
//! incrementally: a full scan seeds it once at startup, publications and
//! deletions adjust it, and the per-tick check stays scan-free (a scan happens
//! only when the tree is over its target or the hourly recount is due).
//! Every enforcement scan re-seeds the counter, folding in index sidecars
//! the web process published since the previous scan; the hourly recount
//! guarantees that sidecar growth is folded in even when the incremental
//! counter alone never crosses the budget. Writer temporaries count toward the
//! budget too.
//!
//! One pass deletes against a deficit computed once at its start and counts
//! every confirmed removal as reclaimed, because unlinked-but-open files
//! release their blocks only when the last reader closes them; in `auto` mode
//! those bytes stay pending until `statvfs` shows the drop, so held
//! descriptors never trigger deleting extra history.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kronika_layout::{LayoutLimits, LayoutSnapshot, SegmentAddress, TemporaryKind, WriterOwner};

use crate::config::RetentionConfig;
use crate::logging::{LogLevel, field, log_event};

/// Period of the size re-check independent of publications.
const TICK: Duration = Duration::from_mins(1);

/// Period of the authoritative fixed-mode recount scan. It bounds how long
/// growth invisible to the incremental counter (index sidecars, failed
/// publication stats) can keep the tree over budget without triggering a scan.
const RECOUNT_PERIOD: Duration = Duration::from_hours(1);

/// Live rotation state for one running collector.
pub(crate) struct Rotation {
    config: RetentionConfig,
    limits: LayoutLimits,
    /// Incremental size of every countable file except the active journal,
    /// seeded by the startup scan, adjusted on publications and deletions, and
    /// re-seeded from every enforcement scan (index sidecars appear outside
    /// the collector's publication path, so the counter under-counts between
    /// scans).
    non_journal_bytes: u64,
    /// Instant of the last periodic size re-check.
    last_tick: Instant,
    /// Instant of the last authoritative recount scan.
    last_recount: Instant,
    /// Instant of the last degradation event; throttles it to one per tick.
    last_degradation: Option<Instant>,
    /// `auto` only: bytes logically freed whose physical release `statvfs` has
    /// not shown yet (readers may hold the unlinked files open).
    pending_reclaim: u64,
    /// `auto` only: partition used bytes at the previous observation, the
    /// baseline for crediting physical drops against `pending_reclaim`.
    last_observed_used: Option<u64>,
}

/// One file rotation may delete, in priority order.
enum Victim {
    /// A stale writer ZMS publication temporary.
    Temporary(kronika_layout::TemporaryObject),
    /// An index sidecar with no sibling ZMS.
    OrphanIndex(SegmentAddress),
    /// A finished segment (its ZMS and sibling IDX).
    Segment(SegmentAddress),
}

impl Victim {
    /// Whether this victim's bytes are part of the incremental tree counter.
    ///
    /// Orphan indexes carry no scanned size, so they may not adjust the
    /// counter on removal.
    const fn counted_in_tree(&self) -> bool {
        matches!(self, Self::Temporary(_) | Self::Segment(_))
    }

    const fn log_kind(&self) -> &'static str {
        match self {
            Self::Temporary(_) => "temporary",
            Self::OrphanIndex(_) => "orphan_index",
            Self::Segment(_) => "segment",
        }
    }

    fn log_path(&self) -> String {
        match self {
            Self::Temporary(temporary) => {
                format!("{}/{}", temporary.address.day, temporary.file_name())
            }
            Self::OrphanIndex(address) => format!("{}/{}", address.day, address.idx_name()),
            Self::Segment(address) => format!("{}/{}", address.day, address.zms_name()),
        }
    }

    const fn segment_id(&self) -> i64 {
        match self {
            Self::Temporary(temporary) => temporary.address.id.get(),
            Self::OrphanIndex(address) | Self::Segment(address) => address.id.get(),
        }
    }
}

impl Rotation {
    /// Seeds the incremental size counter from one full scan (the restart
    /// recount) and returns `None` when rotation is disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the seeding scan fails.
    pub(crate) fn new(
        config: Option<RetentionConfig>,
        owner: &WriterOwner,
        limits: LayoutLimits,
        now: Instant,
    ) -> Result<Option<Self>> {
        let Some(config) = config else {
            return Ok(None);
        };
        let snapshot = owner
            .root()
            .scan(limits)
            .context("scan the tree to seed rotation")?;
        let non_journal_bytes = countable_bytes(&snapshot);
        log_event(
            LogLevel::Info,
            "rotation_seed",
            &[
                field("reason", reason_label(config)),
                field("non_journal_bytes", non_journal_bytes),
                field("segments", snapshot.segments.len()),
            ],
        );
        Ok(Some(Self {
            config,
            limits,
            non_journal_bytes,
            last_tick: now,
            last_recount: now,
            last_degradation: None,
            pending_reclaim: 0,
            last_observed_used: None,
        }))
    }

    /// Duration until the next periodic tick is due.
    pub(crate) fn time_until_tick(&self, now: Instant) -> Duration {
        TICK.saturating_sub(now.saturating_duration_since(self.last_tick))
    }

    /// Records that a publication grew the tree by `bytes`.
    pub(crate) const fn record_publication(&mut self, bytes: u64) {
        self.non_journal_bytes = self.non_journal_bytes.saturating_add(bytes);
    }

    /// Enforces the target if a publication happened or the tick came due.
    pub(crate) fn maybe_enforce(
        &mut self,
        owner: &WriterOwner,
        journal_bytes: u64,
        published: bool,
        now: Instant,
    ) {
        let tick_due = self.time_until_tick(now).is_zero();
        if !published && !tick_due {
            return;
        }
        if tick_due {
            self.last_tick = now;
        }
        if let Err(err) = self.enforce(owner, journal_bytes, now) {
            log_event(
                LogLevel::Warn,
                "rotation_failure",
                &[field("error", format!("{err:#}"))],
            );
        }
    }

    /// One enforcement pass.
    ///
    /// The pass checks the target scan-free, then, only when over the target
    /// or when the hourly recount is due, scans once, re-seeds the counter
    /// from that scan, and deletes the oldest replaceable data against a
    /// deficit computed once for the pass. Confirmed removals count as
    /// reclaimed immediately: physical release may lag while readers hold the
    /// unlinked files open, and re-reading `statvfs` inside the loop would
    /// mistake that lag for "nothing was freed" and delete extra history.
    /// The first removal error ends the pass: the tree state is ambiguous
    /// after a partial mutation, and the next pass rescans from scratch.
    fn enforce(&mut self, owner: &WriterOwner, journal_bytes: u64, now: Instant) -> Result<()> {
        let recount_due = matches!(self.config, RetentionConfig::Fixed(_))
            && now.saturating_duration_since(self.last_recount) >= RECOUNT_PERIOD;
        let (current, threshold) = self.target(owner, journal_bytes)?;
        if current <= threshold && !recount_due {
            return Ok(());
        }
        let snapshot = owner
            .root()
            .scan(self.limits)
            .context("scan the tree for rotation")?;
        // The scan is fresh truth: re-seed the counter so index sidecars
        // published by the web process since the last scan are counted and the
        // per-victim subtraction below stays consistent with the snapshot.
        self.non_journal_bytes = countable_bytes(&snapshot);
        self.last_recount = now;
        let (current, threshold) = self.target(owner, journal_bytes)?;
        let mut deficit = current.saturating_sub(threshold);
        if deficit == 0 {
            return Ok(());
        }

        let mut freed_total: u64 = 0;
        let mut aborted = false;
        for victim in plan_victims(&snapshot) {
            if deficit == 0 {
                break;
            }
            match remove_victim(owner, &victim) {
                Ok(freed) => {
                    if victim.counted_in_tree() {
                        self.non_journal_bytes = self.non_journal_bytes.saturating_sub(freed);
                    }
                    if matches!(self.config, RetentionConfig::Auto(_)) {
                        self.pending_reclaim = self.pending_reclaim.saturating_add(freed);
                    }
                    deficit = deficit.saturating_sub(freed);
                    freed_total = freed_total.saturating_add(freed);
                    self.log_deletion(
                        &victim,
                        freed,
                        current.saturating_sub(freed_total),
                        threshold,
                    );
                }
                Err(err) => {
                    log_event(
                        LogLevel::Warn,
                        "rotation_delete_failure",
                        &[
                            field("path", victim.log_path()),
                            field("error", format!("{err:#}")),
                        ],
                    );
                    aborted = true;
                    break;
                }
            }
        }
        // Degradation asserts the non-deletable minimum was reached; a pass
        // cut short by a removal error proves no such thing.
        if deficit > 0 && !aborted {
            self.log_degradation(current.saturating_sub(freed_total), threshold, now);
        }
        Ok(())
    }

    /// Current size figure and its threshold under the configured target.
    ///
    /// `Fixed` compares the incremental tree counter (journal, segments,
    /// temporaries) against the byte budget. `auto` compares the
    /// partition's used bytes, less the reclaim still pending physical
    /// release, against the percentage threshold; each observation first
    /// credits any physical drop against that pending figure.
    fn target(&mut self, owner: &WriterOwner, journal_bytes: u64) -> Result<(u64, u64)> {
        match self.config {
            RetentionConfig::Fixed(budget) => Ok((self.tree_bytes(journal_bytes), budget)),
            RetentionConfig::Auto(percent) => {
                let usage = owner
                    .root()
                    .filesystem_usage()
                    .context("read partition usage")?;
                self.pending_reclaim = reconcile_pending_reclaim(
                    self.pending_reclaim,
                    self.last_observed_used,
                    usage.used_bytes,
                );
                self.last_observed_used = Some(usage.used_bytes);
                Ok((
                    usage.used_bytes.saturating_sub(self.pending_reclaim),
                    used_threshold_bytes(usage.total_bytes, percent),
                ))
            }
        }
    }

    const fn tree_bytes(&self, journal_bytes: u64) -> u64 {
        self.non_journal_bytes.saturating_add(journal_bytes)
    }

    fn log_deletion(&self, victim: &Victim, freed: u64, current: u64, threshold: u64) {
        let mut fields = vec![
            field("kind", victim.log_kind()),
            field("path", victim.log_path()),
            field("freed_bytes", freed),
            field("reason", reason_label(self.config)),
            field("current_bytes", current),
            field("threshold_bytes", threshold),
        ];
        {
            let segment_id = victim.segment_id();
            fields.push(field("segment_id", segment_id));
        }
        log_event(LogLevel::Info, "rotation_delete", &fields);
    }

    fn log_degradation(&mut self, current: u64, threshold: u64, now: Instant) {
        let throttled = self
            .last_degradation
            .is_some_and(|last| now.saturating_duration_since(last) < TICK);
        if throttled {
            return;
        }
        self.last_degradation = Some(now);
        log_event(
            LogLevel::Warn,
            "rotation_degraded",
            &[
                field("reason", reason_label(self.config)),
                field("current_bytes", current),
                field("threshold_bytes", threshold),
                field(
                    "detail",
                    "minimum viable storage reached; collection continues",
                ),
            ],
        );
    }
}

/// Selects deletion candidates in a fixed order: writer temporaries, then
/// orphan indexes, then finished segments oldest-first. The newest finished
/// segment is never a candidate; together with the active journal it is the
/// non-deletable minimum.
fn plan_victims(snapshot: &LayoutSnapshot) -> Vec<Victim> {
    let mut victims = Vec::new();
    for temporary in &snapshot.temporaries {
        if temporary.kind == TemporaryKind::Zms {
            victims.push(Victim::Temporary(temporary.clone()));
        }
    }
    for orphan in &snapshot.orphan_indexs {
        victims.push(Victim::OrphanIndex(*orphan));
    }
    let deletable_segments = snapshot.segments.len().saturating_sub(1);
    for segment in snapshot.segments.iter().take(deletable_segments) {
        victims.push(Victim::Segment(segment.address));
    }
    victims
}

/// Deletes one victim and returns the bytes it freed.
fn remove_victim(owner: &WriterOwner, victim: &Victim) -> Result<u64> {
    match victim {
        Victim::Temporary(temporary) => {
            let bytes = temporary.identity.len;
            owner
                .remove_temporary(temporary)
                .context("remove a writer temporary")?;
            Ok(bytes)
        }
        Victim::OrphanIndex(address) => owner
            .remove_orphan_index(*address)
            .context("remove an orphan index"),
        Victim::Segment(address) => Ok(owner
            .remove_finished_segment(*address)
            .context("remove a finished segment")?
            .total_bytes()),
    }
}

/// Bytes of every sized file the scan reports: finished segments and writer
/// temporaries. The active journal is excluded (its size is supplied live by
/// the writer's own byte counter), and orphan indexs are excluded because
/// the scan reports no size for them.
fn countable_bytes(snapshot: &LayoutSnapshot) -> u64 {
    let segments = snapshot
        .segments
        .iter()
        .map(|segment| {
            segment
                .zms_bytes
                .saturating_add(segment.idx_bytes.unwrap_or(0))
        })
        .fold(0_u64, u64::saturating_add);
    let temporaries = snapshot
        .temporaries
        .iter()
        .map(|temporary| temporary.identity.len)
        .fold(0_u64, u64::saturating_add);
    segments.saturating_add(temporaries)
}

/// Shrinks the pending logical reclaim by the physical drop the partition has
/// shown since the previous observation.
///
/// Any observed drop is credited to pending reclaim first; a foreign deletion
/// mistaken for ours only makes rotation delete less, never more.
fn reconcile_pending_reclaim(pending: u64, last_observed_used: Option<u64>, used_now: u64) -> u64 {
    let observed_drop = last_observed_used.map_or(0, |last| last.saturating_sub(used_now));
    pending.saturating_sub(observed_drop)
}

/// The byte figure `percent` of `total` maps to.
fn used_threshold_bytes(total: u64, percent: u8) -> u64 {
    u64::try_from(u128::from(total) * u128::from(percent) / 100).unwrap_or(u64::MAX)
}

const fn reason_label(config: RetentionConfig) -> &'static str {
    match config {
        RetentionConfig::Fixed(_) => "budget",
        RetentionConfig::Auto(_) => "auto",
    }
}

#[cfg(test)]
mod tests;
