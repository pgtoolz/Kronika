//! Type `1_107_001`: pressure stall information for one recorded OS scope.

use crate::{Section, Ts};

/// One resource's PSI counters from one host or cgroup pressure snapshot.
///
/// `full_*` fields are `None` for the `cpu` resource, which has no `full` line.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_107_001,
    name = "os_psi",
    semantics = snapshot_full,
    sort_key("resource", "ts"),
    identity("resource")
)]
pub struct OsPsi {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Resource: `0`=cpu, `1`=memory, `2`=io.
    #[column(l)]
    pub resource: u8,
    /// Fraction of time tasks stalled (some) over the last 10 s.
    #[column(g, unit = count)]
    pub some_avg10: f64,
    /// Fraction of time tasks stalled (some) over the last 60 s.
    #[column(g, unit = count)]
    pub some_avg60: f64,
    /// Fraction of time tasks stalled (some) over the last 300 s.
    #[column(g, unit = count)]
    pub some_avg300: f64,
    /// Cumulative stall time (some).
    #[column(c, unit = microseconds)]
    pub some_total: i64,
    /// Fraction of time tasks stalled (full) over the last 10 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg10: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 60 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg60: Option<f64>,
    /// Fraction of time tasks stalled (full) over the last 300 s. `None` for cpu.
    #[column(g, unit = count)]
    pub full_avg300: Option<f64>,
    /// Cumulative stall time (full), microseconds. `None` for cpu.
    #[column(c, unit = microseconds)]
    pub full_total: Option<i64>,
    /// Source scope (`0=host`, `3=container`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_psi.rs"]
mod tests;
