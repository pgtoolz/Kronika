//! Type `1_117_001`: per-NUMA-node memory from
//! `/sys/devices/system/node/node*/meminfo`.

use crate::{Section, Ts};

/// Memory accounting for one NUMA node, in KiB.
///
/// A host with free memory overall can still reclaim hard on one node, and the
/// node-wide totals are the only place that shows it. Row count is the node
/// count, which is one on most machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_117_001,
    name = "os_numa",
    semantics = snapshot_full,
    sort_key("node_id", "ts"),
    identity("node_id")
)]
pub struct OsNuma {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// NUMA node index.
    #[column(l)]
    pub node_id: i32,
    /// Total memory on the node.
    #[column(g, unit = kib)]
    pub mem_total: i64,
    /// Free memory on the node.
    #[column(g, unit = kib)]
    pub mem_free: Option<i64>,
    /// Used memory on the node.
    #[column(g, unit = kib)]
    pub mem_used: Option<i64>,
    /// Page cache on the node.
    #[column(g, unit = kib)]
    pub file_pages: Option<i64>,
    /// Dirty pages on the node.
    #[column(g, unit = kib)]
    pub dirty: Option<i64>,
    /// Pages under writeback on the node.
    #[column(g, unit = kib)]
    pub writeback: Option<i64>,
    /// Anonymous pages on the node.
    #[column(g, unit = kib)]
    pub anon_pages: Option<i64>,
    /// Mapped file pages on the node.
    #[column(g, unit = kib)]
    pub mapped: Option<i64>,
    /// Shared memory on the node.
    #[column(g, unit = kib)]
    pub shmem: Option<i64>,
    /// Slab memory on the node.
    #[column(g, unit = kib)]
    pub slab: Option<i64>,
    /// Reclaimable slab on the node.
    #[column(g, unit = kib)]
    pub s_reclaimable: Option<i64>,
    /// Unreclaimable slab on the node.
    #[column(g, unit = kib)]
    pub s_unreclaim: Option<i64>,
    /// Anonymous transparent huge pages on the node.
    #[column(g, unit = kib)]
    pub anon_huge_pages: Option<i64>,
    /// Total persistent huge pages on the node.
    #[column(g, unit = pages)]
    pub huge_pages_total: Option<i64>,
    /// Free persistent huge pages on the node.
    #[column(g, unit = pages)]
    pub huge_pages_free: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsNuma;
    use crate::{Section, Ts, contract::lint};

    fn row(node_id: i32, dense: bool) -> OsNuma {
        OsNuma {
            ts: Ts(1),
            node_id,
            mem_total: 33_554_432,
            mem_free: dense.then_some(1_048_576),
            mem_used: dense.then_some(32_505_856),
            file_pages: dense.then_some(16_777_216),
            dirty: dense.then_some(128),
            writeback: dense.then_some(0),
            anon_pages: dense.then_some(8_388_608),
            mapped: dense.then_some(1_024),
            shmem: dense.then_some(4_096),
            slab: dense.then_some(524_288),
            s_reclaimable: dense.then_some(262_144),
            s_unreclaim: dense.then_some(262_144),
            anon_huge_pages: dense.then_some(0),
            huge_pages_total: dense.then_some(0),
            huge_pages_free: dense.then_some(0),
            scope: 0,
        }
    }

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsNuma::CONTRACT]), Ok(()));
    }

    #[test]
    fn roundtrip_across_nodes_and_sparse_kernels() {
        crate::assert_roundtrips(&[row(0, true), row(1, false)]);
    }
}
