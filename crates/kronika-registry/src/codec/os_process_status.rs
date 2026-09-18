//! Type `1_101_001`: extended `/proc/PID/status` process metrics.

use crate::{Section, Ts};

/// Less frequent process status fields from `/proc/PID/status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_101_001,
    name = "os_process_status",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct OsProcessStatus {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Process ID.
    #[column(l)]
    pub pid: i32,
    /// Process start timestamp, unix microseconds.
    #[column(l)]
    pub starttime: Ts,
    /// Data segment size, kB.
    #[column(g, unit = count)]
    pub vm_data: i64,
    /// Stack size, kB.
    #[column(g, unit = count)]
    pub vm_stk: i64,
    /// Shared library size, kB.
    #[column(g, unit = count)]
    pub vm_lib: i64,
    /// Locked memory, kB.
    #[column(g, unit = count)]
    pub vm_lck: i64,
    /// Page table memory, kB.
    #[column(g, unit = pages)]
    pub vm_pte: i64,
    /// Peak virtual memory, kB.
    #[column(g, unit = count)]
    pub vm_peak: i64,
    /// Peak resident set size, kB.
    #[column(g, unit = count)]
    pub vm_hwm: i64,
    /// Thread count from `status`.
    #[column(g, unit = count)]
    pub threads: u32,
    /// Allocated file descriptor table size.
    #[column(g, unit = count)]
    pub fdsize: u32,
    /// Voluntary context switches.
    #[column(c, unit = count)]
    pub voluntary_ctxt_switches: i64,
    /// Nonvoluntary context switches.
    #[column(c, unit = count)]
    pub nonvoluntary_ctxt_switches: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_process_status.rs"]
mod tests;
