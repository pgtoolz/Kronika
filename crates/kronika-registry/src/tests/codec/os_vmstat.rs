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
