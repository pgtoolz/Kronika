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
#[path = "../tests/segments/age.rs"]
mod tests;
