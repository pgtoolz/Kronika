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
