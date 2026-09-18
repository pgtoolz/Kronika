//! Type `1_105_001`: system load averages from `/proc/loadavg`.

use crate::{Section, Ts};

/// System load averages from `/proc/loadavg`.
///
/// One row per snapshot; `running`/`total` are the process counts from the
/// `running/total` token.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_105_001,
    name = "os_loadavg",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsLoadavg {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// 1-minute load average.
    #[column(g, unit = none)]
    pub load1: f64,
    /// 5-minute load average.
    #[column(g, unit = none)]
    pub load5: f64,
    /// 15-minute load average.
    #[column(g, unit = none)]
    pub load15: f64,
    /// Runnable processes at collection time.
    #[column(g, unit = count)]
    pub running: i32,
    /// Total threads/processes at collection time.
    #[column(g, unit = count)]
    pub total: i32,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_loadavg.rs"]
mod tests;
