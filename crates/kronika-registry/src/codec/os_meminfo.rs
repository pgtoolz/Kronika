//! Type `1_104_001`: memory stats from `/proc/meminfo`.

use crate::{Section, Ts};

/// Memory statistics from the `/proc/meminfo` singleton.
///
/// All size fields are raw KiB values as reported by the kernel.
/// Fields absent on the running kernel decode as `None`, never as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_104_001,
    name = "os_meminfo",
    semantics = snapshot_full,
    sort_key("ts")
)]
pub struct OsMeminfo {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Total usable RAM.
    #[column(g, unit = kib)]
    pub mem_total: i64,
    /// Free (completely unused) RAM.
    #[column(g, unit = kib)]
    pub mem_free: Option<i64>,
    /// Estimate of available RAM for new allocations.
    #[column(g, unit = kib)]
    pub mem_available: Option<i64>,
    /// In-memory block device cache (buffers).
    #[column(g, unit = kib)]
    pub buffers: Option<i64>,
    /// Page cache (excluding `SwapCached`).
    #[column(g, unit = kib)]
    pub cached: Option<i64>,
    /// Total swap space.
    #[column(g, unit = kib)]
    pub swap_total: Option<i64>,
    /// Unused swap space.
    #[column(g, unit = kib)]
    pub swap_free: Option<i64>,
    /// Active (recently used) memory.
    #[column(g, unit = kib)]
    pub active: Option<i64>,
    /// Inactive (candidate for reclaim) memory.
    #[column(g, unit = kib)]
    pub inactive: Option<i64>,
    /// Dirty pages waiting to be written back.
    #[column(g, unit = kib)]
    pub dirty: Option<i64>,
    /// Pages currently being written back.
    #[column(g, unit = kib)]
    pub writeback: Option<i64>,
    /// Total slab memory (reclaimable + unreclaimable).
    #[column(g, unit = kib)]
    pub slab: Option<i64>,
    /// Slab memory reclaimable under pressure.
    #[column(g, unit = kib)]
    pub s_reclaimable: Option<i64>,
    /// Slab memory not reclaimable.
    #[column(g, unit = kib)]
    pub s_unreclaim: Option<i64>,
    /// Non-file-backed pages mapped into page tables.
    #[column(g, unit = kib)]
    pub anon_pages: Option<i64>,
    /// Files mapped into memory.
    #[column(g, unit = kib)]
    pub mapped: Option<i64>,
    /// Memory used by shared memory (`tmpfs`).
    #[column(g, unit = kib)]
    pub shmem: Option<i64>,
    /// Memory used by page tables.
    #[column(g, unit = kib)]
    pub page_tables: Option<i64>,
    /// Upper limit of committed virtual memory.
    #[column(g, unit = kib)]
    pub commit_limit: Option<i64>,
    /// Total committed virtual memory.
    #[column(g, unit = kib)]
    pub committed_as: Option<i64>,
    /// Total huge pages in the pool.
    #[column(g, unit = pages)]
    pub huge_pages_total: Option<i64>,
    /// Free huge pages in the pool.
    #[column(g, unit = pages)]
    pub huge_pages_free: Option<i64>,
    /// Size of one huge page.
    #[column(g, unit = kib)]
    pub hugepagesize: Option<i64>,
    /// Swap pages also held in RAM.
    #[column(g, unit = kib)]
    pub swap_cached: Option<i64>,
    /// Unevictable pages.
    #[column(g, unit = kib)]
    pub unevictable: Option<i64>,
    /// Pages locked into RAM by `mlock`.
    #[column(g, unit = kib)]
    pub mlocked: Option<i64>,
    /// Anonymous transparent huge pages.
    #[column(g, unit = kib)]
    pub anon_huge_pages: Option<i64>,
    /// Shared memory backed by huge pages.
    #[column(g, unit = kib)]
    pub shmem_huge_pages: Option<i64>,
    /// Kernel stacks.
    #[column(g, unit = kib)]
    pub kernel_stack: Option<i64>,
    /// Per-CPU allocator memory.
    #[column(g, unit = kib)]
    pub percpu: Option<i64>,
    /// Block-device bounce buffers.
    #[column(g, unit = kib)]
    pub bounce: Option<i64>,
    /// NFS pages written to the server but not yet committed.
    #[column(g, unit = kib)]
    pub nfs_unstable: Option<i64>,
    /// Writeback pages held on FUSE temporary storage.
    #[column(g, unit = kib)]
    pub writeback_tmp: Option<i64>,
    /// Huge pages reserved but not yet allocated.
    #[column(g, unit = pages)]
    pub huge_pages_rsvd: Option<i64>,
    /// Huge pages above the configured pool size.
    #[column(g, unit = pages)]
    pub huge_pages_surp: Option<i64>,
    /// Compressed swap pool footprint.
    #[column(g, unit = kib)]
    pub zswap: Option<i64>,
    /// Original size of the pages held in the compressed swap pool.
    #[column(g, unit = kib)]
    pub zswapped: Option<i64>,
    /// Used vmalloc area.
    #[column(g, unit = kib)]
    pub vmalloc_used: Option<i64>,
    /// Source scope (`0=host`). See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsMeminfo;
    use crate::{Section, Ts, VerifiedSection, lint};

    fn full_row(ts: i64) -> OsMeminfo {
        OsMeminfo {
            ts: Ts(ts),
            mem_total: 16_777_216,
            mem_free: Some(4_096_000),
            mem_available: Some(8_192_000),
            buffers: Some(512_000),
            cached: Some(3_145_728),
            swap_total: Some(8_388_608),
            swap_free: Some(8_000_000),
            active: Some(6_291_456),
            inactive: Some(2_097_152),
            dirty: Some(1024),
            writeback: Some(0),
            slab: Some(524_288),
            s_reclaimable: Some(262_144),
            s_unreclaim: Some(262_144),
            anon_pages: Some(4_194_304),
            mapped: Some(1_048_576),
            shmem: Some(32_768),
            page_tables: Some(16_384),
            commit_limit: Some(12_582_912),
            committed_as: Some(10_485_760),
            huge_pages_total: Some(0),
            huge_pages_free: Some(0),
            hugepagesize: Some(2048),
            swap_cached: Some(4_096),
            unevictable: Some(0),
            mlocked: Some(0),
            anon_huge_pages: Some(2_097_152),
            shmem_huge_pages: Some(0),
            kernel_stack: Some(16_384),
            percpu: Some(8_192),
            bounce: Some(0),
            nfs_unstable: Some(0),
            writeback_tmp: Some(0),
            huge_pages_rsvd: Some(0),
            huge_pages_surp: Some(0),
            zswap: Some(0),
            zswapped: Some(0),
            vmalloc_used: Some(65_536),
            scope: 0,
        }
    }

    fn sparse_row(ts: i64) -> OsMeminfo {
        OsMeminfo {
            ts: Ts(ts),
            mem_total: 8_388_608,
            mem_free: Some(1_000_000),
            mem_available: None,
            buffers: None,
            cached: None,
            swap_total: None,
            swap_free: None,
            active: None,
            inactive: None,
            dirty: None,
            writeback: None,
            slab: None,
            s_reclaimable: None,
            s_unreclaim: None,
            anon_pages: None,
            mapped: None,
            shmem: None,
            page_tables: None,
            commit_limit: None,
            committed_as: None,
            huge_pages_total: None,
            huge_pages_free: None,
            hugepagesize: None,
            swap_cached: None,
            unevictable: None,
            mlocked: None,
            anon_huge_pages: None,
            shmem_huge_pages: None,
            kernel_stack: None,
            percpu: None,
            bounce: None,
            nfs_unstable: None,
            writeback_tmp: None,
            huge_pages_rsvd: None,
            huge_pages_surp: None,
            zswap: None,
            zswapped: None,
            vmalloc_used: None,
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsMeminfo::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsMeminfo::CONTRACT;
        assert_eq!(c.type_id.get(), 1_104_001);
        assert_eq!(c.sort_key, ["ts"]);
        assert_eq!(c.column("mem_total").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("s_reclaimable").map(|col| col.nullable),
            Some(true)
        );
        assert_eq!(c.column("s_unreclaim").map(|col| col.nullable), Some(true));
    }

    #[test]
    fn roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[full_row(1_000), sparse_row(2_000)]);
    }

    #[test]
    fn nulls_survive_distinct_from_zero() {
        let bytes = OsMeminfo::encode(&[sparse_row(5)]).expect("encode");
        let decoded = OsMeminfo::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0].mem_available, None);
        assert_eq!(decoded[0].slab, None);
        assert_eq!(decoded[0].s_reclaimable, None);
        assert_eq!(decoded[0].s_unreclaim, None);
        assert_eq!(decoded[0].dirty, None);
        assert_eq!(decoded[0].writeback, None);
    }
}
