//! Type `1_103_001`: misc counters from `/proc/stat`.

use crate::{Section, Ts};

/// Miscellaneous kernel counters from the `/proc/stat` singleton lines.
///
/// Collected once per snapshot; `btime` is unix microseconds from the
/// `secs * 1_000_000` conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_103_001,
    name = "os_stat",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsStat {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Context switches since boot.
    #[column(c, unit = count)]
    pub ctxt: i64,
    /// Processes forked since boot.
    #[column(c, unit = count)]
    pub processes: i64,
    /// Processes in runnable state at collection time.
    #[column(g, unit = count)]
    pub procs_running: i64,
    /// Processes blocked waiting for I/O at collection time.
    #[column(g, unit = count)]
    pub procs_blocked: i64,
    /// Kernel boot time, unix microseconds.
    #[column(g, unit = microseconds)]
    pub btime: Ts,
    /// Hardware interrupts serviced since boot, all lines summed.
    #[column(c, unit = count)]
    pub intr_total: Option<i64>,
    /// Software interrupts serviced since boot, all vectors summed.
    #[column(c, unit = count)]
    pub softirq_total: Option<i64>,
    /// Seconds since boot (`/proc/uptime` field 1).
    #[column(g, unit = microseconds)]
    pub uptime_us: Option<i64>,
    /// Cumulative idle time of all cores (`/proc/uptime` field 2),
    /// microseconds.
    #[column(g, unit = microseconds)]
    pub idle_us: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_stat.rs"]
mod tests;
