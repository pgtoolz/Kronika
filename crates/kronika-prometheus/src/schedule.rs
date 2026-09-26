//! Interval scheduling decisions.
//!
//! Pure time arithmetic: the collector loop asks which metric is due and the
//! cache applies the staleness threshold. Both compare against the start of
//! the previous run, successful or not, so a failing metric is retried on
//! its normal cadence (EXE-7).

/// Whether a metric is due: at least `interval_s` elapsed since the previous
/// run started. A clock step backwards leaves the metric not due.
#[must_use]
pub fn due(last_start_ms: i64, interval_s: u64, now_ms: i64) -> bool {
    u64::try_from(now_ms - last_start_ms).is_ok_and(|elapsed| elapsed >= interval_s * 1000)
}

/// Exposition staleness threshold: `max(10 min, 2 × interval)`.
#[must_use]
pub const fn stale_threshold_ms(interval_s: u64) -> u64 {
    // Ord::max is not const-stable yet
    let twice = 2 * interval_s * 1000;
    if twice > 600_000 { twice } else { 600_000 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_at_and_after_interval() {
        assert!(due(0, 60, 60_000));
        assert!(due(0, 60, 60_001));
        assert!(!due(0, 60, 59_999));
        assert!(due(0, 0, 0)); // zero-interval metrics are always due
    }

    #[test]
    fn failed_runs_do_not_shorten_the_interval() {
        // same predicate after an error: exactly one interval after the last
        // start, regardless of how long the failing query took
        assert!(!due(10_000, 60, 10_000 + 59_000));
        assert!(due(10_000, 60, 10_000 + 60_000));
    }

    #[test]
    fn threshold_never_below_ten_minutes() {
        assert_eq!(stale_threshold_ms(60), 600_000);
        assert_eq!(stale_threshold_ms(300), 600_000);
        assert_eq!(stale_threshold_ms(3600), 7_200_000);
    }
}
