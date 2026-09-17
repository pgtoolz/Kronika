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
