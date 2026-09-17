use super::{
    CPU_FRAME, WAVE_PERIOD, WORKER_TICK, cpu_busy_time, hourly_cpu_millis, hourly_payload_bytes,
    payload_bytes_for_tick, quarters_at,
};
use std::collections::BTreeSet;
use std::time::Duration;

#[test]
fn the_wave_has_six_visible_phases_and_repeats() {
    let phases: Vec<u64> = (0..6)
        .map(|phase| quarters_at(Duration::from_secs(phase * 10)))
        .collect();
    assert_eq!(phases, [1, 2, 3, 4, 3, 2]);
    for second in 0..WAVE_PERIOD.as_secs() {
        assert_eq!(
            quarters_at(Duration::from_secs(second)),
            quarters_at(Duration::from_secs(second + WAVE_PERIOD.as_secs()))
        );
    }
}

#[test]
fn every_cpu_frame_has_sleep_time_and_the_default_varies() {
    let values: BTreeSet<Duration> = (0..6)
        .map(|phase| cpu_busy_time(12, Duration::from_secs(phase * 10)))
        .collect();
    assert_eq!(values.len(), 4);
    assert!(values.iter().all(|value| *value < CPU_FRAME));
    assert_eq!(values.first(), Some(&Duration::from_millis(3)));
    assert_eq!(values.last(), Some(&Duration::from_millis(12)));
}

#[test]
fn one_hour_matches_the_documented_default_budgets() {
    let ticks = 3_600_000 / WORKER_TICK.as_millis();
    let payload: u64 = (0..ticks)
        .map(|tick| {
            let elapsed_ms = tick * WORKER_TICK.as_millis();
            payload_bytes_for_tick(
                32,
                Duration::from_millis(u64::try_from(elapsed_ms).unwrap()),
            )
        })
        .sum();
    assert_eq!(payload, 73_728_000);
    assert_eq!(payload, hourly_payload_bytes(32));

    let frames = 3_600_000 / CPU_FRAME.as_millis();
    let busy: Duration = (0..frames)
        .map(|frame| {
            let elapsed_ms = frame * CPU_FRAME.as_millis();
            cpu_busy_time(
                12,
                Duration::from_millis(u64::try_from(elapsed_ms).unwrap()),
            )
        })
        .sum();
    assert_eq!(busy, Duration::from_secs(270));
    assert_eq!(busy.as_millis(), u128::from(hourly_cpu_millis(12)));
}

#[test]
fn a_tick_never_exceeds_the_configured_peak_rate() {
    for phase in 0..6 {
        let bytes = payload_bytes_for_tick(256, Duration::from_secs(phase * 10));
        assert!(bytes <= 256 * 1024 / 4);
    }
}
