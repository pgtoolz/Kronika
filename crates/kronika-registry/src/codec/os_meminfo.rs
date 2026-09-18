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
#[path = "../tests/codec/os_meminfo.rs"]
mod tests;
