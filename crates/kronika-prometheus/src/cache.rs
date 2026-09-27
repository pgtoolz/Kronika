//! Per-database result cache with the staleness rule.
//!
//! Scrape reads the cache; background collection replaces entries wholly.
//! A result older than `max(10 min, 2 × interval)` is not exposed — the
//! pgwatch sink uses a flat 10 minutes, Kronika keeps slow metrics visible.

use std::collections::BTreeMap;

use crate::executor::INSTANCE_UP_METRIC;
use crate::measurement::SampleSet;
use crate::schedule::stale_threshold_ms;

/// Default `instance_up` interval when the preset omits it; the row is
/// exposed for every discovered database regardless of the preset.
pub const INSTANCE_UP_INTERVAL_S: u64 = 60;

/// Cached state of one database: last fetch per metric plus `instance_up`.
///
/// Each entry carries its preset interval; the interval drives both
/// scheduling and the staleness threshold.
#[derive(Debug, Default, PartialEq)]
pub struct DbCache {
    instance_up: Option<Entry>,
    metrics: BTreeMap<String, Entry>,
}

/// One cached fetch with its interval.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Collection interval seconds from the active preset.
    pub interval_s: u64,
    /// The cached samples.
    pub set: SampleSet,
}

/// One cache entry surfaced by [`DbCache::snapshot`].
#[derive(Debug, PartialEq)]
pub struct SnapshotRow {
    /// Metric name (or `instance_up`).
    pub metric: String,
    /// The cached samples with interval.
    pub entry: Entry,
}

impl DbCache {
    /// Replaces the `instance_up` state (derived connection verdict).
    /// `interval_s` is the preset interval or the default.
    pub fn store_instance_up(&mut self, set: SampleSet, interval_s: u64) {
        self.instance_up = Some(Entry { interval_s, set });
    }

    /// Replaces the cached result of `metric` wholly.
    pub fn store(&mut self, metric: &str, entry: Entry) {
        self.metrics.insert(metric.to_owned(), entry);
    }

    /// Non-stale entries for exposition: `instance_up` first, then metrics in
    /// name order. `now_ms` is the render time, captured after the SQL of
    /// its pass. A timestamp later than that (an epoch slightly ahead of
    /// the pass start) stays fresh: it is a real execution timestamp, not a
    /// clock step, and dropping it would hide a just-fetched metric.
    #[must_use]
    pub fn snapshot(&self, now_ms: i64) -> Vec<SnapshotRow> {
        let fresh =
            |e: &Entry| !stale(now_ms, e.set.timestamp_ms, stale_threshold_ms(e.interval_s));
        let mut rows = Vec::with_capacity(self.metrics.len() + 1);
        if let Some(entry) = &self.instance_up
            && fresh(entry)
        {
            rows.push(SnapshotRow {
                metric: INSTANCE_UP_METRIC.to_owned(),
                entry: entry.clone(),
            });
        }
        for (metric, entry) in &self.metrics {
            if metric != INSTANCE_UP_METRIC && fresh(entry) {
                rows.push(SnapshotRow {
                    metric: metric.clone(),
                    entry: entry.clone(),
                });
            }
        }
        rows
    }
}

fn stale(now_ms: i64, fetched_ms: i64, threshold_ms: u64) -> bool {
    // a fetch at or after the render time is fresh (real execution stamp)
    u64::try_from(now_ms.saturating_sub(fetched_ms)).is_ok_and(|age| age > threshold_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::instance_up_sample_set;

    const MINUTE: i64 = 60_000;
    const TEN_MINUTES: i64 = 600_000;

    fn entry(interval_s: u64, timestamp_ms: i64) -> Entry {
        Entry {
            interval_s,
            set: SampleSet {
                timestamp_ms,
                samples: Vec::new(),
                errors: 0,
            },
        }
    }

    #[test]
    fn instance_up_is_always_first_and_fresh_by_ping() {
        let mut cache = DbCache::default();
        cache.store_instance_up(
            instance_up_sample_set("db", true, 1_000),
            INSTANCE_UP_INTERVAL_S,
        );
        let rows = cache.snapshot(1_000 + 9 * MINUTE);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric, "instance_up");
        // a ping that keeps failing refreshes the timestamp, so a reachable
        // 10-minute staleness never fires for it; at the boundary it holds
        let rows = cache.snapshot(1_000 + TEN_MINUTES);
        assert_eq!(rows.len(), 1);
        let rows = cache.snapshot(1_000 + TEN_MINUTES + 1);
        assert!(rows.is_empty());
    }

    #[test]
    fn slow_metrics_survive_past_ten_minutes() {
        let mut cache = DbCache::default();
        // db_size at 300s: threshold stays max(10min, 600s) = 10min
        cache.store("db_size", entry(300, 0));
        assert_eq!(cache.snapshot(TEN_MINUTES).len(), 1);
        assert!(cache.snapshot(TEN_MINUTES + 1).is_empty());

        // an hourly metric: threshold becomes 2h
        cache.store("reco_add_index", entry(3600, 0));
        assert_eq!(cache.snapshot(TEN_MINUTES + 1).len(), 1);
        assert_eq!(cache.snapshot(2 * 3_600_000).len(), 1);
        assert!(cache.snapshot(2 * 3_600_000 + 1).is_empty());
    }

    #[test]
    fn future_timestamp_stays_fresh() {
        // an epoch later than the render time is a real execution stamp,
        // not a clock step: a just-fetched metric must not disappear
        let mut cache = DbCache::default();
        cache.store("m", entry(60, 5_000));
        assert_eq!(cache.snapshot(1_000).len(), 1);
        assert!(cache.snapshot(5_000 + 10 * 60_000 + 1).is_empty());
    }

    #[test]
    fn store_replaces_wholly() {
        let mut cache = DbCache::default();
        cache.store("a", entry(60, 10));
        cache.store("a", entry(60, 20));
        cache.store("b", entry(60, 10));
        let rows = cache.snapshot(30);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].metric, "a");
        assert_eq!(rows[0].entry.set.timestamp_ms, 20);
    }

    #[test]
    fn snapshot_orders_instance_up_then_names() {
        let mut cache = DbCache::default();
        cache.store_instance_up(instance_up_sample_set("db", true, 0), 60);
        for m in ["wal", "db_size", "db_stats"] {
            cache.store(m, entry(60, 0));
        }
        let names: Vec<String> = cache.snapshot(0).into_iter().map(|r| r.metric).collect();
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            ["instance_up", "db_size", "db_stats", "wal"]
        );
    }
}
