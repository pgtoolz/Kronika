use std::time::Duration;

pub(super) fn until_next_phase(seed: u64, period: Duration, utc: Duration) -> Duration {
    let period = period.as_nanos();
    if period == 0 {
        return Duration::ZERO;
    }
    let phase = u128::from(seed) % period;
    let position = utc.as_nanos() % period;
    let delay = if position < phase {
        phase - position
    } else {
        period - (position - phase)
    };
    Duration::new(
        u64::try_from(delay / 1_000_000_000).expect("delay is at most the configured period"),
        u32::try_from(delay % 1_000_000_000).expect("subsecond nanoseconds fit u32"),
    )
}

#[cfg(test)]
mod tests {
    use super::until_next_phase;
    use std::time::Duration;

    #[test]
    fn phase_grid_has_subsecond_precision_and_a_strictly_future_boundary() {
        let period = Duration::from_secs(10);
        assert_eq!(
            until_next_phase(2_000_000_000, period, Duration::from_millis(101_500)),
            Duration::from_millis(500)
        );
        assert_eq!(
            until_next_phase(2_000_000_000, period, Duration::from_secs(102)),
            period
        );
        assert_eq!(
            until_next_phase(2_500_000_000, period, Duration::from_secs(102)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn zero_and_full_configured_duration_range_do_not_overflow() {
        assert_eq!(
            until_next_phase(7, Duration::ZERO, Duration::from_secs(500)),
            Duration::ZERO
        );
        for seed in [0, 1, u64::MAX] {
            let period = Duration::from_secs(u64::MAX);
            let delay = until_next_phase(seed, period, Duration::from_hours(500_000));
            assert!(!delay.is_zero());
            assert!(delay <= period);
        }
    }
}
