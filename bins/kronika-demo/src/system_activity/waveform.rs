//! One deterministic waveform shared by CPU, disk, and loopback workers.

use std::time::Duration;

pub(super) const WAVE_PERIOD: Duration = Duration::from_mins(1);
pub(super) const WORKER_TICK: Duration = Duration::from_millis(250);
pub(super) const CPU_FRAME: Duration = Duration::from_millis(100);

const PHASE_SECONDS: u64 = 10;
const QUARTERS: [u64; 6] = [1, 2, 3, 4, 3, 2];
const NANOS_PER_SECOND: u128 = 1_000_000_000;

pub(super) fn quarters_at(elapsed: Duration) -> u64 {
    let within_period = elapsed.as_secs() % WAVE_PERIOD.as_secs();
    let phase = usize::try_from(within_period / PHASE_SECONDS).unwrap_or_default();
    QUARTERS.get(phase).copied().unwrap_or(QUARTERS[0])
}

pub(super) fn payload_bytes_for_tick(peak_kib_per_s: u64, elapsed: Duration) -> u64 {
    let numerator = u128::from(peak_kib_per_s)
        * 1024
        * u128::from(quarters_at(elapsed))
        * WORKER_TICK.as_nanos();
    u64::try_from(numerator / (4 * NANOS_PER_SECOND)).unwrap_or(u64::MAX)
}

pub(super) fn cpu_busy_time(peak_percent: u64, elapsed: Duration) -> Duration {
    let numerator =
        CPU_FRAME.as_nanos() * u128::from(peak_percent) * u128::from(quarters_at(elapsed));
    let nanos = u64::try_from(numerator / 400).unwrap_or(u64::MAX);
    Duration::from_nanos(nanos)
}

pub(super) const fn hourly_payload_bytes(peak_kib_per_s: u64) -> u64 {
    peak_kib_per_s
        .saturating_mul(1024)
        .saturating_mul(3_600)
        .saturating_mul(5)
        / 8
}

pub(super) const fn hourly_cpu_millis(peak_percent: u64) -> u64 {
    3_600_000_u64.saturating_mul(peak_percent).saturating_mul(5) / (8 * 100)
}

#[cfg(test)]
mod tests;
