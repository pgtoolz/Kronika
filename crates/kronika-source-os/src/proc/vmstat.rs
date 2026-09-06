//! Parse `/proc/vmstat` into the `1_106_001` registry section.

use kronika_registry::Ts;
use kronika_registry::os_vmstat::OsVmstat;

use super::stat::ParseError;

/// Parsed fields from a single `/proc/vmstat` snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmstatRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Pages paged in from disk.
    pub pgpgin: Option<i64>,
    /// Pages paged out to disk.
    pub pgpgout: Option<i64>,
    /// Swap pages swapped in.
    pub pswpin: Option<i64>,
    /// Swap pages swapped out.
    pub pswpout: Option<i64>,
    /// Minor page faults (no disk I/O needed).
    pub pgfault: Option<i64>,
    /// Major page faults (disk I/O required).
    pub pgmajfault: Option<i64>,
    /// Pages stolen by kswapd during reclaim.
    pub pgsteal_kswapd: Option<i64>,
    /// Pages stolen directly during reclaim.
    pub pgsteal_direct: Option<i64>,
    /// Pages scanned by kswapd.
    pub pgscan_kswapd: Option<i64>,
    /// Pages scanned directly.
    pub pgscan_direct: Option<i64>,
    /// OOM killer invocations.
    pub oom_kill: Option<i64>,
    /// Pages allocated from the normal zone.
    pub pgalloc_normal: Option<i64>,
    /// Pages moved to the inactive list on refill.
    pub pgrefill: Option<i64>,
    /// Pages promoted to the active list.
    pub pgactivate: Option<i64>,
    /// Pages demoted to the inactive list.
    pub pgdeactivate: Option<i64>,
    /// Pages scanned by khugepaged during reclaim.
    pub pgscan_khugepaged: Option<i64>,
    /// Pages stolen by khugepaged during reclaim.
    pub pgsteal_khugepaged: Option<i64>,
    /// Allocation stalls that entered direct reclaim, all zones summed.
    pub allocstall: Option<i64>,
    /// Allocation stalls that entered direct compaction.
    pub compact_stall: Option<i64>,
    /// Pages migrated between NUMA nodes.
    pub numa_pages_migrated: Option<i64>,
    /// Page migrations that succeeded.
    pub pgmigrate_success: Option<i64>,
    /// Page migrations that failed.
    pub pgmigrate_fail: Option<i64>,
    /// Transparent huge pages allocated on fault.
    pub thp_fault_alloc: Option<i64>,
    /// Transparent huge pages built by khugepaged.
    pub thp_collapse_alloc: Option<i64>,
    /// Refaults of evicted page-cache pages.
    pub workingset_refault_file: Option<i64>,
    /// Refaults of evicted anonymous pages.
    pub workingset_refault_anon: Option<i64>,
    /// Refaulted pages restored to the active list.
    pub workingset_restore_file: Option<i64>,
    /// Shadow nodes reclaimed from the working-set tracker.
    pub workingset_nodereclaim: Option<i64>,
    /// Pages read ahead from swap.
    pub swap_ra: Option<i64>,
    /// Swap read-ahead pages that were used.
    pub swap_ra_hit: Option<i64>,
}

/// Parse `/proc/vmstat` content into a [`VmstatRow`].
///
/// Each line has the form `key value` (space-separated, no colon, no unit).
/// All fields are optional; absent keys decode as `None`, never as zero.
///
/// # Errors
///
/// Returns [`ParseError`] when a present value cannot be parsed as `i64`.
pub fn parse_vmstat(content: &str, ts: i64) -> Result<VmstatRow, ParseError> {
    let mut row = VmstatRow {
        ts,
        ..VmstatRow::default()
    };

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value_str)) = (parts.next(), parts.next()) else {
            continue;
        };
        let value = value_str
            .parse::<i64>()
            .map_err(|e| ParseError(format!("/proc/vmstat {key:?}: {e}")))?;
        assign(&mut row, key, value);
    }

    Ok(row)
}

/// Store one `/proc/vmstat` value; keys this build does not read are ignored.
fn assign(row: &mut VmstatRow, key: &str, value: i64) {
    match key {
        "pgpgin" => row.pgpgin = Some(value),
        "pgpgout" => row.pgpgout = Some(value),
        "pswpin" => row.pswpin = Some(value),
        "pswpout" => row.pswpout = Some(value),
        "pgfault" => row.pgfault = Some(value),
        "pgmajfault" => row.pgmajfault = Some(value),
        "pgsteal_kswapd" => row.pgsteal_kswapd = Some(value),
        "pgsteal_direct" => row.pgsteal_direct = Some(value),
        "pgscan_kswapd" => row.pgscan_kswapd = Some(value),
        "pgscan_direct" => row.pgscan_direct = Some(value),
        "oom_kill" => row.oom_kill = Some(value),
        "pgalloc_normal" => row.pgalloc_normal = Some(value),
        "pgrefill" => row.pgrefill = Some(value),
        "pgactivate" => row.pgactivate = Some(value),
        "pgdeactivate" => row.pgdeactivate = Some(value),
        "pgscan_khugepaged" => row.pgscan_khugepaged = Some(value),
        "pgsteal_khugepaged" => row.pgsteal_khugepaged = Some(value),
        "compact_stall" => row.compact_stall = Some(value),
        "numa_pages_migrated" => row.numa_pages_migrated = Some(value),
        "pgmigrate_success" => row.pgmigrate_success = Some(value),
        "pgmigrate_fail" => row.pgmigrate_fail = Some(value),
        "thp_fault_alloc" => row.thp_fault_alloc = Some(value),
        "thp_collapse_alloc" => row.thp_collapse_alloc = Some(value),
        "workingset_refault_file" => row.workingset_refault_file = Some(value),
        "workingset_refault_anon" => row.workingset_refault_anon = Some(value),
        "workingset_restore_file" => row.workingset_restore_file = Some(value),
        "workingset_nodereclaim" => row.workingset_nodereclaim = Some(value),
        "swap_ra" => row.swap_ra = Some(value),
        "swap_ra_hit" => row.swap_ra_hit = Some(value),
        // The kernel splits direct-reclaim stalls per zone (allocstall_dma,
        // allocstall_normal, ...); only the total is useful and the zone set
        // varies by machine.
        other if other.starts_with("allocstall") => {
            row.allocstall = Some(row.allocstall.unwrap_or(0) + value);
        }
        _ => {}
    }
}

impl VmstatRow {
    /// Registry row for `1_106_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsVmstat {
        OsVmstat {
            ts: Ts(self.ts),
            pgpgin: self.pgpgin,
            pgpgout: self.pgpgout,
            pswpin: self.pswpin,
            pswpout: self.pswpout,
            pgfault: self.pgfault,
            pgmajfault: self.pgmajfault,
            pgsteal_kswapd: self.pgsteal_kswapd,
            pgsteal_direct: self.pgsteal_direct,
            pgscan_kswapd: self.pgscan_kswapd,
            pgscan_direct: self.pgscan_direct,
            oom_kill: self.oom_kill,
            pgalloc_normal: self.pgalloc_normal,
            pgrefill: self.pgrefill,
            pgactivate: self.pgactivate,
            pgdeactivate: self.pgdeactivate,
            pgscan_khugepaged: self.pgscan_khugepaged,
            pgsteal_khugepaged: self.pgsteal_khugepaged,
            allocstall: self.allocstall,
            compact_stall: self.compact_stall,
            numa_pages_migrated: self.numa_pages_migrated,
            pgmigrate_success: self.pgmigrate_success,
            pgmigrate_fail: self.pgmigrate_fail,
            thp_fault_alloc: self.thp_fault_alloc,
            thp_collapse_alloc: self.thp_collapse_alloc,
            workingset_refault_file: self.workingset_refault_file,
            workingset_refault_anon: self.workingset_refault_anon,
            workingset_restore_file: self.workingset_restore_file,
            workingset_nodereclaim: self.workingset_nodereclaim,
            swap_ra: self.swap_ra,
            swap_ra_hit: self.swap_ra_hit,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_vmstat;

    const FULL_SAMPLE: &str = "\
pgpgin 1000000\n\
pgpgout 2000000\n\
pswpin 0\n\
pswpout 0\n\
pgfault 5000000\n\
pgmajfault 1024\n\
pgsteal_kswapd 512000\n\
pgsteal_direct 4096\n\
pgscan_kswapd 768000\n\
pgscan_direct 8192\n\
oom_kill 0\n";

    const SPARSE_SAMPLE: &str = "\
pgpgin 100\n\
pgpgout 200\n\
nr_free_pages 12345\n";

    #[test]
    fn parses_full_sample() {
        let row = parse_vmstat(FULL_SAMPLE, 9_999).expect("parse");
        assert_eq!(row.ts, 9_999);
        assert_eq!(row.pgpgin, Some(1_000_000));
        assert_eq!(row.pgpgout, Some(2_000_000));
        assert_eq!(row.pswpin, Some(0));
        assert_eq!(row.pswpout, Some(0));
        assert_eq!(row.pgfault, Some(5_000_000));
        assert_eq!(row.pgmajfault, Some(1024));
        assert_eq!(row.pgsteal_kswapd, Some(512_000));
        assert_eq!(row.pgsteal_direct, Some(4096));
        assert_eq!(row.pgscan_kswapd, Some(768_000));
        assert_eq!(row.pgscan_direct, Some(8192));
        assert_eq!(row.oom_kill, Some(0));
    }

    #[test]
    fn missing_oom_kill_yields_none() {
        let row = parse_vmstat(SPARSE_SAMPLE, 1).expect("parse");
        assert_eq!(row.pgpgin, Some(100));
        assert_eq!(row.pgpgout, Some(200));
        assert_eq!(row.pswpin, None);
        assert_eq!(row.pswpout, None);
        assert_eq!(row.pgfault, None);
        assert_eq!(row.pgmajfault, None);
        assert_eq!(row.pgsteal_kswapd, None);
        assert_eq!(row.pgsteal_direct, None);
        assert_eq!(row.pgscan_kswapd, None);
        assert_eq!(row.pgscan_direct, None);
        assert_eq!(row.oom_kill, None);
    }

    #[test]
    fn to_section_carries_all_floor_fields_and_scope() {
        let row = parse_vmstat(FULL_SAMPLE, 9_999).expect("parse");
        let section = row.to_section(1);
        assert_eq!(section.ts.0, 9_999);
        assert_eq!(section.pgpgin, Some(1_000_000));
        assert_eq!(section.pgpgout, Some(2_000_000));
        assert_eq!(section.pswpin, Some(0));
        assert_eq!(section.pswpout, Some(0));
        assert_eq!(section.pgfault, Some(5_000_000));
        assert_eq!(section.pgmajfault, Some(1024));
        assert_eq!(section.pgsteal_kswapd, Some(512_000));
        assert_eq!(section.pgsteal_direct, Some(4096));
        assert_eq!(section.pgscan_kswapd, Some(768_000));
        assert_eq!(section.pgscan_direct, Some(8192));
        assert_eq!(section.oom_kill, Some(0));
        assert_eq!(section.scope, 1);
    }
}
