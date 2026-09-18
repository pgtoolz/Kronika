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
