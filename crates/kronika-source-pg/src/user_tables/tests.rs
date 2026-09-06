use super::{
    UserTablesRow, UserTablesVersion, to_v1, to_v2, to_v3, to_v4, user_tables_query,
    user_tables_version,
};
use crate::test_intern as fake_intern;

fn sample_row() -> UserTablesRow {
    UserTablesRow {
        ts: 2_000,
        datid: 16_400,
        datname: "appdb".into(),
        relid: 16_499,
        schemaname: "public".to_owned(),
        relname: "orders".to_owned(),
        tablespace_oid: Some(1_663),
        tablespace: Some("pg_default".to_owned()),
        seq_scan: 12,
        seq_tup_read: 900,
        idx_scan: Some(4_000),
        idx_tup_fetch: Some(3_800),
        n_tup_ins: 500,
        n_tup_upd: 200,
        n_tup_del: 30,
        n_tup_hot_upd: 120,
        n_tup_newpage_upd: Some(8),
        n_live_tup: 10_000,
        n_dead_tup: 400,
        n_mod_since_analyze: 700,
        n_ins_since_vacuum: Some(250),
        vacuum_count: 1,
        autovacuum_count: 9,
        analyze_count: 1,
        autoanalyze_count: 7,
        last_vacuum: Some(1_100),
        last_autovacuum: Some(1_200),
        last_analyze: Some(1_300),
        last_autoanalyze: Some(1_400),
        last_seq_scan: Some(1_500),
        last_idx_scan: Some(1_600),
        total_vacuum_time: Some(35.5),
        total_autovacuum_time: Some(410.0),
        total_analyze_time: Some(12.0),
        total_autoanalyze_time: Some(80.0),
        main_fork_bytes: 8_192_000,
        toast_bytes: Some(65_536),
        toast_n_live_tup: Some(20),
        toast_n_dead_tup: Some(2),
        toast_last_autovacuum: Some(1_700),
        xid_age: Some(150_000_000),
        mxid_age: Some(5_000_000),
        reltuples: 9_900,
        heap_blks_read: 300,
        heap_blks_hit: 90_000,
        idx_blks_read: Some(40),
        idx_blks_hit: Some(9_000),
        toast_blks_read: Some(1),
        toast_blks_hit: Some(50),
        tidx_blks_read: Some(0),
        tidx_blks_hit: Some(60),
    }
}

/// A table with no indexes and no TOAST relation, as the server reports it.
fn bare_row() -> UserTablesRow {
    UserTablesRow {
        idx_scan: None,
        idx_tup_fetch: None,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
        ..sample_row()
    }
}

#[test]
fn version_follows_catalog_changes() {
    assert_eq!(user_tables_version(10), UserTablesVersion::V1);
    assert_eq!(user_tables_version(12), UserTablesVersion::V1);
    assert_eq!(user_tables_version(13), UserTablesVersion::V2);
    assert_eq!(user_tables_version(15), UserTablesVersion::V2);
    assert_eq!(user_tables_version(16), UserTablesVersion::V3);
    assert_eq!(user_tables_version(17), UserTablesVersion::V3);
    assert_eq!(user_tables_version(18), UserTablesVersion::V4);
}

#[test]
fn a_query_asks_only_for_columns_its_server_has() {
    assert!(!user_tables_query(UserTablesVersion::V1).contains("n_ins_since_vacuum"));
    assert!(user_tables_query(UserTablesVersion::V2).contains("n_ins_since_vacuum"));
    assert!(!user_tables_query(UserTablesVersion::V2).contains("last_seq_scan"));
    assert!(user_tables_query(UserTablesVersion::V3).contains("n_tup_newpage_upd"));
    assert!(user_tables_query(UserTablesVersion::V3).contains("last_idx_scan"));
    assert!(!user_tables_query(UserTablesVersion::V3).contains("total_autovacuum_time"));
    assert!(user_tables_query(UserTablesVersion::V4).contains("total_autovacuum_time"));
}

#[test]
fn every_query_carries_the_marker_and_the_views_it_needs() {
    for version in [
        UserTablesVersion::V1,
        UserTablesVersion::V2,
        UserTablesVersion::V3,
        UserTablesVersion::V4,
    ] {
        let sql = user_tables_query(version);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("pg_stat_user_tables"), "{sql}");
        assert!(sql.contains("pg_statio_user_tables"), "{sql}");
        assert!(sql.contains("reltoastrelid"), "{sql}");
        assert!(sql.contains("CASE WHEN c.relkind = 'p' THEN NULL"), "{sql}");
        assert!(sql.contains("pg_catalog.age(c.relfrozenxid)"), "{sql}");
        assert!(sql.contains("pg_catalog.mxid_age(c.relminmxid)"), "{sql}");
        assert!(sql.contains("d.dattablespace"), "{sql}");
        for catalog in [
            "pg_catalog.pg_stat_user_tables",
            "pg_catalog.pg_statio_user_tables",
            "pg_catalog.pg_stat_all_tables",
            "pg_catalog.pg_class",
            "pg_catalog.pg_database",
            "pg_catalog.pg_tablespace",
        ] {
            assert!(sql.contains(catalog), "{sql}");
        }
    }
}

#[test]
fn to_v4_maps_every_column_and_interns_the_names() {
    let r = to_v4(&sample_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.datname, fake_intern(b"appdb").unwrap());
    assert_eq!(r.relname, fake_intern(b"orders").unwrap());
    assert_eq!(r.tablespace_oid, Some(1_663));
    assert_eq!(r.tablespace, Some(fake_intern(b"pg_default").unwrap()));
    assert_eq!(r.seq_scan, 12);
    assert_eq!(r.idx_scan, Some(4_000));
    assert_eq!(r.n_tup_newpage_upd, 8);
    assert_eq!(r.n_ins_since_vacuum, 250);
    assert_eq!(r.last_autovacuum.map(|ts| ts.0), Some(1_200));
    assert!((r.total_autovacuum_time - 410.0).abs() < f64::EPSILON);
    assert_eq!(r.toast_bytes, Some(65_536));
    assert_eq!(r.xid_age, Some(150_000_000));
    assert_eq!(r.tidx_blks_hit, Some(60));
}

#[test]
fn a_table_without_indexes_or_toast_keeps_its_nulls() {
    let r = to_v4(&bare_row(), fake_intern).expect("intern");
    assert_eq!(r.idx_scan, None);
    assert_eq!(r.idx_blks_hit, None);
    assert_eq!(r.toast_bytes, None);
    assert_eq!(r.toast_last_autovacuum, None);
    assert_eq!(r.tidx_blks_read, None);
}

#[test]
fn a_partitioned_parent_keeps_null_transaction_ages_in_every_layout() {
    let row = UserTablesRow {
        tablespace_oid: None,
        tablespace: None,
        xid_age: None,
        mxid_age: None,
        ..sample_row()
    };
    let v1 = to_v1(&row, fake_intern).expect("intern");
    let v2 = to_v2(&row, fake_intern).expect("intern");
    let v3 = to_v3(&row, fake_intern).expect("intern");
    let v4 = to_v4(&row, fake_intern).expect("intern");
    assert_eq!((v1.xid_age, v1.mxid_age), (None, None));
    assert_eq!((v2.xid_age, v2.mxid_age), (None, None));
    assert_eq!((v3.xid_age, v3.mxid_age), (None, None));
    assert_eq!((v4.xid_age, v4.mxid_age), (None, None));
    assert_eq!((v1.tablespace_oid, v1.tablespace), (None, None));
    assert_eq!((v2.tablespace_oid, v2.tablespace), (None, None));
    assert_eq!((v3.tablespace_oid, v3.tablespace), (None, None));
    assert_eq!((v4.tablespace_oid, v4.tablespace), (None, None));
}

#[test]
fn a_missing_tablespace_name_keeps_the_effective_oid() {
    let row = UserTablesRow {
        tablespace: None,
        ..sample_row()
    };
    let mapped = to_v4(&row, fake_intern).expect("intern");
    assert_eq!(mapped.tablespace_oid, Some(1_663));
    assert_eq!(mapped.tablespace, None);
}

#[test]
fn a_column_the_server_never_sent_lands_as_zero_not_as_a_guess() {
    let mut row = sample_row();
    row.n_ins_since_vacuum = None;
    row.n_tup_newpage_upd = None;
    row.total_vacuum_time = None;
    let r = to_v4(&row, fake_intern).expect("intern");
    assert_eq!(r.n_ins_since_vacuum, 0);
    assert_eq!(r.n_tup_newpage_upd, 0);
    assert!(r.total_vacuum_time.abs() < f64::EPSILON);
}

#[test]
fn to_v3_carries_the_last_scan_times_but_no_cumulative_ones() {
    let r = to_v3(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.last_seq_scan.map(|ts| ts.0), Some(1_500));
    assert_eq!(r.n_tup_newpage_upd, 8);
}

#[test]
fn to_v2_carries_the_insert_counter_since_the_last_vacuum() {
    let r = to_v2(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.n_ins_since_vacuum, 250);
    assert_eq!(r.n_dead_tup, 400);
}

#[test]
fn to_v1_maps_the_base_layout() {
    let r = to_v1(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.relid, 16_499);
    assert_eq!(r.autovacuum_count, 9);
    assert_eq!(r.reltuples, 9_900);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v4(&sample_row(), |_| Err("full")), Err("full"));
}
