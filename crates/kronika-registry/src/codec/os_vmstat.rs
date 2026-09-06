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
mod tests {
    use super::OsVmstat;
    use crate::{Section, Ts, VerifiedSection, lint};

    fn full_row(ts: i64) -> OsVmstat {
        OsVmstat {
            ts: Ts(ts),
            pgpgin: Some(1_000_000),
            pgpgout: Some(2_000_000),
            pswpin: Some(0),
            pswpout: Some(0),
            pgfault: Some(5_000_000),
            pgmajfault: Some(1024),
            pgsteal_kswapd: Some(512_000),
            pgsteal_direct: Some(4096),
            pgscan_kswapd: Some(768_000),
            pgscan_direct: Some(8192),
            oom_kill: Some(0),
            pgalloc_normal: Some(9_000_000),
            pgrefill: Some(1_000),
            pgactivate: Some(2_000),
            pgdeactivate: Some(3_000),
            pgscan_khugepaged: Some(0),
            pgsteal_khugepaged: Some(0),
            allocstall: Some(7),
            compact_stall: Some(1),
            numa_pages_migrated: Some(0),
            pgmigrate_success: Some(11),
            pgmigrate_fail: Some(0),
            thp_fault_alloc: Some(5),
            thp_collapse_alloc: Some(2),
            workingset_refault_file: Some(100),
            workingset_refault_anon: Some(0),
            workingset_restore_file: Some(20),
            workingset_nodereclaim: Some(0),
            swap_ra: Some(0),
            swap_ra_hit: Some(0),
            scope: 0,
        }
    }

    fn sparse_row(ts: i64) -> OsVmstat {
        OsVmstat {
            ts: Ts(ts),
            pgpgin: Some(100),
            pgpgout: Some(200),
            pswpin: None,
            pswpout: None,
            pgfault: None,
            pgmajfault: None,
            pgsteal_kswapd: None,
            pgsteal_direct: None,
            pgscan_kswapd: None,
            pgscan_direct: None,
            oom_kill: None,
            pgalloc_normal: None,
            pgrefill: None,
            pgactivate: None,
            pgdeactivate: None,
            pgscan_khugepaged: None,
            pgsteal_khugepaged: None,
            allocstall: None,
            compact_stall: None,
            numa_pages_migrated: None,
            pgmigrate_success: None,
            pgmigrate_fail: None,
            thp_fault_alloc: None,
            thp_collapse_alloc: None,
            workingset_refault_file: None,
            workingset_refault_anon: None,
            workingset_restore_file: None,
            workingset_nodereclaim: None,
            swap_ra: None,
            swap_ra_hit: None,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsVmstat::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsVmstat::CONTRACT;
        assert_eq!(c.type_id.get(), 1_106_001);
        assert_eq!(c.sort_key, ["ts"]);
        assert_eq!(c.column("pgpgin").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("oom_kill").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("scope").map(|col| col.nullable), Some(false));
    }

    #[test]
    fn roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[full_row(1_000), sparse_row(2_000)]);
    }

    #[test]
    fn nulls_survive_distinct_from_zero() {
        let bytes = OsVmstat::encode(&[sparse_row(5)]).expect("encode");
        let decoded = OsVmstat::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].pswpin, None);
        assert_eq!(decoded[0].pswpout, None);
        assert_eq!(decoded[0].pgfault, None);
        assert_eq!(decoded[0].pgmajfault, None);
        assert_eq!(decoded[0].pgsteal_kswapd, None);
        assert_eq!(decoded[0].pgsteal_direct, None);
        assert_eq!(decoded[0].pgscan_kswapd, None);
        assert_eq!(decoded[0].pgscan_direct, None);
        assert_eq!(decoded[0].oom_kill, None);
    }
}
