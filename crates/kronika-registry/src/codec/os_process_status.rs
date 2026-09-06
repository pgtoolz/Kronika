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
mod tests {
    use super::OsProcessStatus;
    use crate::{Section, Ts, contract::lint};

    fn row(ts: i64, pid: i32) -> OsProcessStatus {
        OsProcessStatus {
            ts: Ts(ts),
            pid,
            starttime: Ts(1_700_000_000_000_000 + i64::from(pid)),
            vm_data: 10,
            vm_stk: 11,
            vm_lib: 12,
            vm_lck: 13,
            vm_pte: 14,
            vm_peak: 15,
            vm_hwm: 16,
            threads: 2,
            fdsize: 64,
            voluntary_ctxt_switches: 100,
            nonvoluntary_ctxt_switches: 7,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsProcessStatus::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsProcessStatus::CONTRACT;
        assert_eq!(c.type_id.get(), 1_101_001);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[row(1, 10), row(2, 11)]);
    }
}
