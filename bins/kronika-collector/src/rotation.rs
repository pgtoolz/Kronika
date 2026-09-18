//! Delete old storage files after publication or once per minute.
//!
//! A fixed budget limits the data tree; `auto` limits partition usage.
//! Both keep the active WAL and newest segment. File selection is in `files`.

mod files;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kronika_layout::{LayoutLimits, WriterOwner};

use files::{countable_bytes, deletion_candidates};

use crate::config::RetentionConfig;
use crate::logging::{LogLevel, field, log_event};

/// Recheck retention once per minute, even when no segment is published.
const CHECK_INTERVAL: Duration = Duration::from_mins(1);
/// Recount hourly in fixed mode to include indexes created by the web process.
const RECOUNT_INTERVAL: Duration = Duration::from_hours(1);

pub(crate) struct Rotation {
    config: RetentionConfig,
    limits: LayoutLimits,
    /// Last scan plus publications minus deletions; WAL size comes from the writer.
    non_journal_bytes: u64,
    last_tick: Instant,
    last_recount: Instant,
    /// Throttle the warning about protected files to once per check interval.
    last_degradation: Option<Instant>,
    /// Auto mode: unlinked bytes minus subsequent drops in partition usage.
    pending_reclaim: u64,
    last_observed_used: Option<u64>,
}

impl Rotation {
    /// Scan once to seed the byte counter. `None` disables retention.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial scan fails.
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
        CHECK_INTERVAL.saturating_sub(now.saturating_duration_since(self.last_tick))
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

    fn enforce(&mut self, owner: &WriterOwner, journal_bytes: u64, now: Instant) -> Result<()> {
        let recount_due = matches!(self.config, RetentionConfig::Fixed(_))
            && now.saturating_duration_since(self.last_recount) >= RECOUNT_INTERVAL;
        let (current, threshold) = self.usage(owner, journal_bytes)?;
        if current <= threshold && !recount_due {
            return Ok(());
        }

        let snapshot = owner
            .root()
            .scan(self.limits)
            .context("scan the tree for rotation")?;
        // Include files published outside the collector, notably index sidecars.
        self.non_journal_bytes = countable_bytes(&snapshot);
        self.last_recount = now;
        let (mut current, threshold) = self.usage(owner, journal_bytes)?;

        // Credit each confirmed unlink once. Re-reading partition usage here
        // could delete extra history while readers still hold the removed files.
        for candidate in deletion_candidates(&snapshot) {
            if current <= threshold {
                break;
            }
            let freed = match candidate.remove(owner) {
                Ok(freed) => freed,
                Err(error) => {
                    log_event(
                        LogLevel::Warn,
                        "rotation_delete_failure",
                        &[
                            field("path", candidate.path()),
                            field("error", format!("{error:#}")),
                        ],
                    );
                    // Removal can fail halfway through a ZMS/IDX pair. Stop;
                    // an error does not mean only protected files remain.
                    return Ok(());
                }
            };
            if candidate.counted_in_tree() {
                self.non_journal_bytes = self.non_journal_bytes.saturating_sub(freed);
            }
            if matches!(self.config, RetentionConfig::Auto(_)) {
                self.pending_reclaim = self.pending_reclaim.saturating_add(freed);
            }
            current = current.saturating_sub(freed);
            candidate.log_removed(reason_label(self.config), freed, current, threshold);
        }
        if current > threshold {
            self.log_degradation(current, threshold, now);
        }
        Ok(())
    }

    /// Compare tree bytes with a fixed budget, or partition usage with a percentage.
    /// Auto mode credits unlinks before the partition reports lower usage.
    fn usage(&mut self, owner: &WriterOwner, journal_bytes: u64) -> Result<(u64, u64)> {
        match self.config {
            RetentionConfig::Fixed(budget) => {
                Ok((self.non_journal_bytes.saturating_add(journal_bytes), budget))
            }
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

    fn log_degradation(&mut self, current: u64, threshold: u64, now: Instant) {
        let throttled = self
            .last_degradation
            .is_some_and(|last| now.saturating_duration_since(last) < CHECK_INTERVAL);
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

/// Discharge pending unlinks by the observed drop in partition usage.
/// Partition-wide changes cannot be attributed to individual files.
fn reconcile_pending_reclaim(pending: u64, last_observed_used: Option<u64>, used_now: u64) -> u64 {
    let observed_drop = last_observed_used.map_or(0, |last| last.saturating_sub(used_now));
    pending.saturating_sub(observed_drop)
}

/// Round the percentage down, using u128 so large filesystems cannot overflow.
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
#[path = "tests/rotation.rs"]
mod tests;
