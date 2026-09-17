//! Type `1_102_001`: CPU time from `/proc/stat` `cpu`/`cpuN` lines.

use crate::{Section, Ts};

/// One CPU's cumulative scheduler ticks.
///
/// The aggregate `cpu` line uses `cpu_id = -1`; per-cpu lines carry their
/// index. All time fields are raw scheduler ticks — the reader converts
/// through `clock_ticks_per_sec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_102_001,
    name = "os_cpu",
    semantics = snapshot_full,
    sort_key("cpu_id", "ts"),
    identity("cpu_id")
)]
pub struct OsCpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// `-1` for the aggregate `cpu` line, else the CPU index.
    #[column(l)]
    pub cpu_id: i32,
    /// Ticks in user mode.
    #[column(c, unit = jiffies)]
    pub user: i64,
    /// Ticks in user mode with low priority (nice).
    #[column(c, unit = jiffies)]
    pub nice: i64,
    /// Ticks in system (kernel) mode.
    #[column(c, unit = jiffies)]
    pub system: i64,
    /// Ticks idle.
    #[column(c, unit = jiffies)]
    pub idle: i64,
    /// Ticks waiting for I/O to complete.
    #[column(c, unit = jiffies)]
    pub iowait: i64,
    /// Ticks serving hardware interrupts.
    #[column(c, unit = jiffies)]
    pub irq: i64,
    /// Ticks serving software interrupts.
    #[column(c, unit = jiffies)]
    pub softirq: i64,
    /// Ticks stolen by a hypervisor.
    #[column(c, unit = jiffies)]
    pub steal: i64,
    /// Ticks spent running a virtual CPU for a guest OS.
    #[column(c, unit = jiffies)]
    pub guest: i64,
    /// Ticks spent running a niced guest.
    #[column(c, unit = jiffies)]
    pub guest_nice: i64,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_cpu.rs"]
mod tests;
