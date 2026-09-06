//! Health components and their additive penalties.

/// Whether one optional source contributes a known penalty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePenalty {
    /// The source was not enabled for this collection.
    Disabled,
    /// The source was enabled, but this snapshot has no usable value.
    Unknown,
    /// A penalty from `0` to `100`.
    Known(u8),
}

/// What one snapshot reported in `os_psi`: cumulative stall time per resource,
/// microseconds since the counters started.
///
/// The `some` line of each resource, never `full`: the question is whether
/// anyone lost time, not whether everything stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stall {
    /// `/proc/pressure/cpu`, `some`.
    pub cpu: i64,
    /// `/proc/pressure/memory`, `some`.
    pub memory: i64,
    /// `/proc/pressure/io`, `some`.
    pub io: i64,
}

/// Health over the interval between two snapshots, `0` to `100`.
///
/// The value is the share of the interval in which nothing was waiting for the
/// most contended resource. `None` where it cannot be computed: the interval is
/// not positive, or a counter went backwards.
#[must_use]
pub fn health(before: Stall, before_ts: i64, after: Stall, after_ts: i64) -> Option<u8> {
    let elapsed = after_ts.checked_sub(before_ts).filter(|span| *span > 0)?;
    let cpu = stalled(before.cpu, after.cpu)?;
    let memory = stalled(before.memory, after.memory)?;
    let io = stalled(before.io, after.io)?;
    let worst = cpu.max(memory).max(io);
    Some(percent_left(worst, elapsed))
}

/// `PostgreSQL` pressure from the number of backends that are `active` in one
/// `pg_stat_activity` snapshot.
///
/// One effective CPU supplies two service slots: one backend can execute while
/// another sends results to its client. Pressure starts only above that bound.
/// `None` means capacity is nonpositive or nonfinite.
#[must_use]
pub fn postgres_penalty(active: u32, effective_cpus: f64) -> Option<u8> {
    if !effective_cpus.is_finite() || effective_cpus <= 0.0 {
        return None;
    }
    let active = f64::from(active);
    let service_slots = 2.0 * effective_cpus;
    if active <= service_slots {
        return Some(0);
    }
    let rounded = (100.0 * (active - service_slots) / active).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded penalty is in 0..=100"
    )]
    Some(rounded as u8)
}

/// Add OS and `PostgreSQL` penalties into one `0..=100` value.
///
/// OS health is required. An enabled source with an unknown value makes the
/// result unknown; a disabled source contributes nothing.
#[must_use]
pub fn overall_health(os_health: Option<u8>, postgres: SourcePenalty) -> Option<u8> {
    let os_penalty = 100_u8.saturating_sub(os_health?.min(100));
    let postgres = penalty(postgres)?;
    let total = u16::from(os_penalty).saturating_add(u16::from(postgres));
    Some(100_u8.saturating_sub(u8::try_from(total.min(100)).unwrap_or(100)))
}

fn penalty(value: SourcePenalty) -> Option<u8> {
    match value {
        SourcePenalty::Disabled => Some(0),
        SourcePenalty::Unknown => None,
        SourcePenalty::Known(value) => Some(value.min(100)),
    }
}

/// Stall microseconds between two readings of one counter, or `None` when it
/// went backwards.
fn stalled(before: i64, after: i64) -> Option<i64> {
    after.checked_sub(before).filter(|delta| *delta >= 0)
}

/// `100` minus the share `stalled` takes of `elapsed`, rounded to the nearest
/// whole percent.
///
/// The share is capped defensively rather than allowed to go negative when an
/// input exceeds the elapsed interval.
fn percent_left(stalled: i64, elapsed: i64) -> u8 {
    let capped = stalled.min(elapsed);
    let half = elapsed / 2;
    // Integer arithmetic throughout: the inputs are microseconds, and a float
    // would only add a rounding story to explain.
    let percent = (capped.saturating_mul(100).saturating_add(half)) / elapsed;
    100_u8.saturating_sub(u8::try_from(percent).unwrap_or(100))
}

#[cfg(test)]
mod tests;
