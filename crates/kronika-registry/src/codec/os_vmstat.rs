//! Type `1_106_001`: paging and swap counters from `/proc/vmstat`.

use crate::{Section, Ts};

/// Paging and swap counters from the `/proc/vmstat` singleton.
///
/// All fields are raw event counts as reported by the kernel.
/// Fields absent on the running kernel decode as `None`, never as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_106_001,
    name = "os_vmstat",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsVmstat {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Pages paged in from disk.
    #[column(c, unit = pages)]
    pub pgpgin: Option<i64>,
    /// Pages paged out to disk.
    #[column(c, unit = pages)]
    pub pgpgout: Option<i64>,
    /// Swap pages swapped in.
    #[column(c, unit = pages)]
    pub pswpin: Option<i64>,
    /// Swap pages swapped out.
    #[column(c, unit = pages)]
    pub pswpout: Option<i64>,
    /// Minor page faults (no disk I/O needed).
    #[column(c, unit = pages)]
    pub pgfault: Option<i64>,
    /// Major page faults (disk I/O required).
    #[column(c, unit = pages)]
    pub pgmajfault: Option<i64>,
    /// Pages stolen by kswapd during reclaim.
    #[column(c, unit = pages)]
    pub pgsteal_kswapd: Option<i64>,
    /// Pages stolen directly during reclaim.
    #[column(c, unit = pages)]
    pub pgsteal_direct: Option<i64>,
    /// Pages scanned by kswapd.
    #[column(c, unit = pages)]
    pub pgscan_kswapd: Option<i64>,
    /// Pages scanned directly.
    #[column(c, unit = pages)]
    pub pgscan_direct: Option<i64>,
    /// OOM killer invocations.
    #[column(c, unit = count)]
    pub oom_kill: Option<i64>,
    /// Pages allocated from the normal zone.
    #[column(c, unit = pages)]
    pub pgalloc_normal: Option<i64>,
    /// Pages moved to the inactive list on refill.
    #[column(c, unit = pages)]
    pub pgrefill: Option<i64>,
    /// Pages promoted to the active list.
    #[column(c, unit = pages)]
    pub pgactivate: Option<i64>,
    /// Pages demoted to the inactive list.
    #[column(c, unit = pages)]
    pub pgdeactivate: Option<i64>,
    /// Pages scanned by khugepaged during reclaim.
    #[column(c, unit = count)]
    pub pgscan_khugepaged: Option<i64>,
    /// Pages stolen by khugepaged during reclaim.
    #[column(c, unit = count)]
    pub pgsteal_khugepaged: Option<i64>,
    /// Allocation stalls that had to enter direct reclaim.
    #[column(c, unit = count)]
    pub allocstall: Option<i64>,
    /// Allocation stalls that had to enter direct compaction.
    #[column(c, unit = count)]
    pub compact_stall: Option<i64>,
    /// Pages migrated between NUMA nodes by automatic balancing.
    #[column(c, unit = pages)]
    pub numa_pages_migrated: Option<i64>,
    /// Page migrations that succeeded.
    #[column(c, unit = pages)]
    pub pgmigrate_success: Option<i64>,
    /// Page migrations that failed.
    #[column(c, unit = pages)]
    pub pgmigrate_fail: Option<i64>,
    /// Transparent huge pages allocated on fault.
    #[column(c, unit = count)]
    pub thp_fault_alloc: Option<i64>,
    /// Transparent huge pages built by khugepaged.
    #[column(c, unit = count)]
    pub thp_collapse_alloc: Option<i64>,
    /// Refaults of pages evicted while still in the working set.
    #[column(c, unit = pages)]
    pub workingset_refault_file: Option<i64>,
    /// Refaults of anonymous pages evicted while still in the working set.
    #[column(c, unit = pages)]
    pub workingset_refault_anon: Option<i64>,
    /// Refaulted pages restored to the active list.
    #[column(c, unit = pages)]
    pub workingset_restore_file: Option<i64>,
    /// Shadow nodes reclaimed from the working-set tracker.
    #[column(c, unit = count)]
    pub workingset_nodereclaim: Option<i64>,
    /// Pages read ahead from swap.
    #[column(c, unit = pages)]
    pub swap_ra: Option<i64>,
    /// Swap read-ahead pages that were used.
    #[column(c, unit = pages)]
    pub swap_ra_hit: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
#[path = "../tests/codec/os_vmstat.rs"]
mod tests;
