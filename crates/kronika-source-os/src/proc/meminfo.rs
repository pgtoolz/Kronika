//! Parse `/proc/meminfo` into the `1_104_001` registry section.

use kronika_registry::Ts;
use kronika_registry::os_meminfo::OsMeminfo;

use super::stat::ParseError;

/// Parsed fields from a single `/proc/meminfo` snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeminfoRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Total usable RAM, KiB. Required; error if absent.
    pub mem_total: i64,
    /// Free (completely unused) RAM, KiB.
    pub mem_free: Option<i64>,
    /// Estimate of available RAM for new allocations, KiB.
    pub mem_available: Option<i64>,
    /// In-memory block device cache, KiB.
    pub buffers: Option<i64>,
    /// Page cache, KiB.
    pub cached: Option<i64>,
    /// Total swap space, KiB.
    pub swap_total: Option<i64>,
    /// Unused swap space, KiB.
    pub swap_free: Option<i64>,
    /// Active (recently used) memory, KiB.
    pub active: Option<i64>,
    /// Inactive (candidate for reclaim) memory, KiB.
    pub inactive: Option<i64>,
    /// Dirty pages waiting to be written back, KiB.
    pub dirty: Option<i64>,
    /// Pages currently being written back, KiB.
    pub writeback: Option<i64>,
    /// Total slab memory, KiB.
    pub slab: Option<i64>,
    /// Slab memory reclaimable under pressure, KiB.
    pub s_reclaimable: Option<i64>,
    /// Slab memory not reclaimable, KiB.
    pub s_unreclaim: Option<i64>,
    /// Non-file-backed pages mapped into page tables, KiB.
    pub anon_pages: Option<i64>,
    /// Files mapped into memory, KiB.
    pub mapped: Option<i64>,
    /// Memory used by shared memory (`tmpfs`), KiB.
    pub shmem: Option<i64>,
    /// Memory used by page tables, KiB.
    pub page_tables: Option<i64>,
    /// Upper limit of committed virtual memory, KiB.
    pub commit_limit: Option<i64>,
    /// Total committed virtual memory, KiB.
    pub committed_as: Option<i64>,
    /// Total huge pages in the pool.
    pub huge_pages_total: Option<i64>,
    /// Free huge pages in the pool.
    pub huge_pages_free: Option<i64>,
    /// Size of one huge page, KiB.
    pub hugepagesize: Option<i64>,
    /// Swap pages also held in RAM, KiB.
    pub swap_cached: Option<i64>,
    /// Unevictable pages, KiB.
    pub unevictable: Option<i64>,
    /// Pages locked into RAM by `mlock`, KiB.
    pub mlocked: Option<i64>,
    /// Anonymous transparent huge pages, KiB.
    pub anon_huge_pages: Option<i64>,
    /// Shared memory backed by huge pages, KiB.
    pub shmem_huge_pages: Option<i64>,
    /// Kernel stacks, KiB.
    pub kernel_stack: Option<i64>,
    /// Per-CPU allocator memory, KiB.
    pub percpu: Option<i64>,
    /// Block-device bounce buffers, KiB.
    pub bounce: Option<i64>,
    /// NFS pages written but not yet committed, KiB.
    pub nfs_unstable: Option<i64>,
    /// FUSE writeback temporary storage, KiB.
    pub writeback_tmp: Option<i64>,
    /// Huge pages reserved but not allocated.
    pub huge_pages_rsvd: Option<i64>,
    /// Huge pages above the configured pool size.
    pub huge_pages_surp: Option<i64>,
    /// Compressed swap pool footprint, KiB.
    pub zswap: Option<i64>,
    /// Original size of pages in the compressed swap pool, KiB.
    pub zswapped: Option<i64>,
    /// Used vmalloc area, KiB.
    pub vmalloc_used: Option<i64>,
}

/// Parse `/proc/meminfo` content into a [`MeminfoRow`].
///
/// Each line has the form `Key:   value kB` (trailing `kB` is optional for
/// counts like `HugePages_Total`). `MemTotal` is required; all other fields
/// default to `None` when the kernel does not emit them.
///
/// # Errors
///
/// Returns [`ParseError`] when `MemTotal` is absent or any present value
/// cannot be parsed as `i64`.
pub fn parse_meminfo(content: &str, ts: i64) -> Result<MeminfoRow, ParseError> {
    let mut row = MeminfoRow {
        ts,
        ..MeminfoRow::default()
    };
    let mut seen_total = false;

    for line in content.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // Value is the first whitespace-separated token after the colon (the
        // trailing `kB` unit token is intentionally ignored).
        let Some(value_str) = rest.split_whitespace().next() else {
            continue;
        };
        let value = value_str
            .parse::<i64>()
            .map_err(|e| ParseError(format!("/proc/meminfo {key:?}: {e}")))?;
        if key == "MemTotal" {
            row.mem_total = value;
            seen_total = true;
            continue;
        }
        assign(&mut row, key, value);
    }

    if seen_total {
        Ok(row)
    } else {
        Err(ParseError("/proc/meminfo: missing MemTotal".to_owned()))
    }
}

/// Store one `/proc/meminfo` value; keys this build does not read are ignored.
fn assign(row: &mut MeminfoRow, key: &str, value: i64) {
    match key {
        "MemFree" => row.mem_free = Some(value),
        "MemAvailable" => row.mem_available = Some(value),
        "Buffers" => row.buffers = Some(value),
        "Cached" => row.cached = Some(value),
        "SwapTotal" => row.swap_total = Some(value),
        "SwapFree" => row.swap_free = Some(value),
        "Active" => row.active = Some(value),
        "Inactive" => row.inactive = Some(value),
        "Dirty" => row.dirty = Some(value),
        "Writeback" => row.writeback = Some(value),
        "Slab" => row.slab = Some(value),
        "SReclaimable" => row.s_reclaimable = Some(value),
        "SUnreclaim" => row.s_unreclaim = Some(value),
        "AnonPages" => row.anon_pages = Some(value),
        "Mapped" => row.mapped = Some(value),
        "Shmem" => row.shmem = Some(value),
        "PageTables" => row.page_tables = Some(value),
        "CommitLimit" => row.commit_limit = Some(value),
        "Committed_AS" => row.committed_as = Some(value),
        "HugePages_Total" => row.huge_pages_total = Some(value),
        "HugePages_Free" => row.huge_pages_free = Some(value),
        "Hugepagesize" => row.hugepagesize = Some(value),
        "SwapCached" => row.swap_cached = Some(value),
        "Unevictable" => row.unevictable = Some(value),
        "Mlocked" => row.mlocked = Some(value),
        "AnonHugePages" => row.anon_huge_pages = Some(value),
        "ShmemHugePages" => row.shmem_huge_pages = Some(value),
        "KernelStack" => row.kernel_stack = Some(value),
        "Percpu" => row.percpu = Some(value),
        "Bounce" => row.bounce = Some(value),
        "NFS_Unstable" => row.nfs_unstable = Some(value),
        "WritebackTmp" => row.writeback_tmp = Some(value),
        "HugePages_Rsvd" => row.huge_pages_rsvd = Some(value),
        "HugePages_Surp" => row.huge_pages_surp = Some(value),
        "Zswap" => row.zswap = Some(value),
        "Zswapped" => row.zswapped = Some(value),
        "VmallocUsed" => row.vmalloc_used = Some(value),
        _ => {}
    }
}

impl MeminfoRow {
    /// Registry row for `1_104_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsMeminfo {
        OsMeminfo {
            ts: Ts(self.ts),
            mem_total: self.mem_total,
            mem_free: self.mem_free,
            mem_available: self.mem_available,
            buffers: self.buffers,
            cached: self.cached,
            swap_total: self.swap_total,
            swap_free: self.swap_free,
            active: self.active,
            inactive: self.inactive,
            dirty: self.dirty,
            writeback: self.writeback,
            slab: self.slab,
            s_reclaimable: self.s_reclaimable,
            s_unreclaim: self.s_unreclaim,
            anon_pages: self.anon_pages,
            mapped: self.mapped,
            shmem: self.shmem,
            page_tables: self.page_tables,
            commit_limit: self.commit_limit,
            committed_as: self.committed_as,
            huge_pages_total: self.huge_pages_total,
            huge_pages_free: self.huge_pages_free,
            hugepagesize: self.hugepagesize,
            swap_cached: self.swap_cached,
            unevictable: self.unevictable,
            mlocked: self.mlocked,
            anon_huge_pages: self.anon_huge_pages,
            shmem_huge_pages: self.shmem_huge_pages,
            kernel_stack: self.kernel_stack,
            percpu: self.percpu,
            bounce: self.bounce,
            nfs_unstable: self.nfs_unstable,
            writeback_tmp: self.writeback_tmp,
            huge_pages_rsvd: self.huge_pages_rsvd,
            huge_pages_surp: self.huge_pages_surp,
            zswap: self.zswap,
            zswapped: self.zswapped,
            vmalloc_used: self.vmalloc_used,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_meminfo;

    const FULL_SAMPLE: &str = "\
MemTotal:       16777216 kB\n\
MemFree:         4096000 kB\n\
MemAvailable:    8192000 kB\n\
Buffers:          512000 kB\n\
Cached:          3145728 kB\n\
SwapCached:            0 kB\n\
Active:          6291456 kB\n\
Inactive:        2097152 kB\n\
Dirty:              1024 kB\n\
Writeback:             0 kB\n\
AnonPages:       4194304 kB\n\
Mapped:          1048576 kB\n\
Shmem:             32768 kB\n\
Slab:             524288 kB\n\
SReclaimable:     262144 kB\n\
SUnreclaim:       262144 kB\n\
PageTables:        16384 kB\n\
SwapTotal:       8388608 kB\n\
SwapFree:        8000000 kB\n\
CommitLimit:    12582912 kB\n\
Committed_AS:   10485760 kB\n\
HugePages_Total:       0\n\
HugePages_Free:        0\n\
Hugepagesize:       2048 kB\n";

    const SPARSE_SAMPLE: &str = "\
MemTotal:        8388608 kB\n\
MemFree:         1000000 kB\n";

    #[test]
    fn parses_full_sample() {
        let row = parse_meminfo(FULL_SAMPLE, 9_999).expect("parse");
        assert_eq!(row.ts, 9_999);
        assert_eq!(row.mem_total, 16_777_216);
        assert_eq!(row.mem_free, Some(4_096_000));
        assert_eq!(row.mem_available, Some(8_192_000));
        assert_eq!(row.buffers, Some(512_000));
        assert_eq!(row.cached, Some(3_145_728));
        assert_eq!(row.swap_total, Some(8_388_608));
        assert_eq!(row.swap_free, Some(8_000_000));
        assert_eq!(row.active, Some(6_291_456));
        assert_eq!(row.inactive, Some(2_097_152));
        assert_eq!(row.dirty, Some(1024));
        assert_eq!(row.writeback, Some(0));
        assert_eq!(row.slab, Some(524_288));
        assert_eq!(row.s_reclaimable, Some(262_144));
        assert_eq!(row.s_unreclaim, Some(262_144));
        assert_eq!(row.anon_pages, Some(4_194_304));
        assert_eq!(row.mapped, Some(1_048_576));
        assert_eq!(row.shmem, Some(32_768));
        assert_eq!(row.page_tables, Some(16_384));
        assert_eq!(row.commit_limit, Some(12_582_912));
        assert_eq!(row.committed_as, Some(10_485_760));
        assert_eq!(row.huge_pages_total, Some(0));
        assert_eq!(row.huge_pages_free, Some(0));
        assert_eq!(row.hugepagesize, Some(2048));
    }

    #[test]
    fn missing_optional_keys_yield_none() {
        let row = parse_meminfo(SPARSE_SAMPLE, 1).expect("parse");
        assert_eq!(row.mem_total, 8_388_608);
        assert_eq!(row.mem_free, Some(1_000_000));
        assert_eq!(row.mem_available, None);
        assert_eq!(row.slab, None);
        assert_eq!(row.s_reclaimable, None);
        assert_eq!(row.s_unreclaim, None);
        assert_eq!(row.dirty, None);
        assert_eq!(row.writeback, None);
        assert_eq!(row.huge_pages_total, None);
    }

    #[test]
    fn missing_mem_total_is_an_error() {
        let no_total = "MemFree: 4096 kB\nMemAvailable: 8192 kB\n";
        assert!(parse_meminfo(no_total, 1).is_err());
    }

    #[test]
    fn to_section_carries_all_floor_fields_and_scope() {
        let row = parse_meminfo(FULL_SAMPLE, 9_999).expect("parse");
        let section = row.to_section(1);
        assert_eq!(section.ts.0, 9_999);
        assert_eq!(section.mem_total, 16_777_216);
        assert_eq!(section.mem_free, Some(4_096_000));
        assert_eq!(section.mem_available, Some(8_192_000));
        assert_eq!(section.buffers, Some(512_000));
        assert_eq!(section.cached, Some(3_145_728));
        assert_eq!(section.slab, Some(524_288));
        assert_eq!(section.s_reclaimable, Some(262_144));
        assert_eq!(section.s_unreclaim, Some(262_144));
        assert_eq!(section.swap_total, Some(8_388_608));
        assert_eq!(section.swap_free, Some(8_000_000));
        assert_eq!(section.dirty, Some(1024));
        assert_eq!(section.writeback, Some(0));
        assert_eq!(section.scope, 1);
    }
}
