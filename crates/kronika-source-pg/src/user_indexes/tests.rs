use super::{
    UserIndexesRow, UserIndexesVersion, to_v1, to_v2, user_indexes_query, user_indexes_version,
};
use crate::test_intern as fake_intern;

fn sample_row() -> UserIndexesRow {
    UserIndexesRow {
        ts: 2_000,
        datid: 16_400,
        datname: "appdb".into(),
        indexrelid: 16_500,
        relid: 16_499,
        schemaname: "public".to_owned(),
        relname: "orders".to_owned(),
        indexrelname: "orders_pkey".to_owned(),
        tablespace_oid: 1_664,
        tablespace: Some("pg_default".to_owned()),
        idx_scan: 120,
        idx_tup_read: 3_400,
        idx_tup_fetch: 3_000,
        main_fork_bytes: 16_384,
        last_idx_scan: Some(1_900),
        indisunique: true,
        indisprimary: true,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname: "btree".to_owned(),
        indexdef: Some(
            "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)".to_owned(),
        ),
        idx_blks_read: 40,
        idx_blks_hit: 9_000,
    }
}

#[test]
fn last_idx_scan_selects_the_layout() {
    assert_eq!(user_indexes_version(10), UserIndexesVersion::V1);
    assert_eq!(user_indexes_version(15), UserIndexesVersion::V1);
    assert_eq!(user_indexes_version(16), UserIndexesVersion::V2);
    assert_eq!(user_indexes_version(18), UserIndexesVersion::V2);
}

#[test]
fn only_the_later_query_asks_for_last_idx_scan() {
    assert!(!user_indexes_query(UserIndexesVersion::V1).contains("last_idx_scan"));
    assert!(user_indexes_query(UserIndexesVersion::V2).contains("last_idx_scan"));
}

#[test]
fn every_query_carries_the_marker_and_the_views_it_needs() {
    for version in [UserIndexesVersion::V1, UserIndexesVersion::V2] {
        let sql = user_indexes_query(version);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("pg_stat_user_indexes"), "{sql}");
        assert!(sql.contains("pg_statio_user_indexes"), "{sql}");
        assert!(sql.contains("pg_catalog.pg_get_indexdef"), "{sql}");
        assert!(sql.contains("pg_catalog.pg_am"), "{sql}");
        assert!(sql.contains("d.dattablespace"), "{sql}");
    }
}

#[test]
fn every_query_bounds_an_individual_index_definition() {
    for version in [UserIndexesVersion::V1, UserIndexesVersion::V2] {
        let sql = user_indexes_query(version);
        assert!(
            sql.contains(
                "pg_catalog.left(pg_catalog.pg_get_indexdef(si.indexrelid), 65536) AS indexdef"
            ),
            "{sql}"
        );
    }
}

#[test]
fn to_v2_maps_every_column_and_interns_the_names() {
    let r = to_v2(&sample_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.datid, 16_400);
    assert_eq!(r.datname, fake_intern(b"appdb").unwrap());
    assert_eq!(r.indexrelname, fake_intern(b"orders_pkey").unwrap());
    assert_eq!(r.tablespace_oid, 1_664);
    assert_eq!(r.tablespace, Some(fake_intern(b"pg_default").unwrap()));
    assert_eq!(r.idx_scan, 120);
    assert_eq!(r.main_fork_bytes, 16_384);
    assert_eq!(r.last_idx_scan.map(|ts| ts.0), Some(1_900));
    assert_eq!(
        r.indexdef,
        Some(
            fake_intern(b"CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)")
                .unwrap()
        )
    );
    assert!(r.indisprimary);
    assert!(!r.indisexclusion);
    assert_eq!(r.idx_blks_hit, 9_000);
}

#[test]
fn an_index_never_scanned_keeps_its_null() {
    let mut row = sample_row();
    row.last_idx_scan = None;
    assert_eq!(
        to_v2(&row, fake_intern).expect("intern").last_idx_scan,
        None
    );
}

#[test]
fn a_concurrently_dropped_index_keeps_a_null_definition_in_both_layouts() {
    let row = UserIndexesRow {
        indexdef: None,
        ..sample_row()
    };
    assert_eq!(to_v1(&row, fake_intern).expect("intern").indexdef, None);
    assert_eq!(to_v2(&row, fake_intern).expect("intern").indexdef, None);
}

#[test]
fn a_missing_tablespace_name_keeps_the_effective_oid() {
    let row = UserIndexesRow {
        tablespace: None,
        ..sample_row()
    };
    let mapped = to_v2(&row, fake_intern).expect("intern");
    assert_eq!(mapped.tablespace_oid, 1_664);
    assert_eq!(mapped.tablespace, None);
}

#[test]
fn to_v1_maps_the_base_layout() {
    let r = to_v1(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.indexrelid, 16_500);
    assert_eq!(r.amname, fake_intern(b"btree").unwrap());
    assert_eq!(r.idx_tup_read, 3_400);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v2(&sample_row(), |_| Err("full")), Err("full"));
}
