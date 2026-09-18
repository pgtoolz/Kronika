use super::{
    ProgressVacuumRow, ProgressVacuumV1Row, ProgressVacuumV2Row, ProgressVacuumV3Row,
    ProgressVacuumVersion, progress_vacuum_query, progress_vacuum_version, to_v1, to_v2, to_v3,
};
use crate::tests::intern as fake_intern;

fn v1_raw() -> ProgressVacuumV1Row {
    ProgressVacuumV1Row {
        ts: 2_000,
        pid: 4242,
        datid: 16_385,
        datname: "appdb".to_owned(),
        relid: 16_384,
        schemaname: Some("public".to_owned()),
        relname: Some("orders".to_owned()),
        is_autovacuum: true,
        phase: "scanning heap".to_owned(),
        heap_blks_total: 10_000,
        heap_blks_scanned: 4_200,
        heap_blks_vacuumed: 4_000,
        index_vacuum_count: 1,
        max_dead_tuples: 291_271,
        num_dead_tuples: 120_000,
    }
}

fn v2_raw() -> ProgressVacuumV2Row {
    ProgressVacuumV2Row {
        ts: 2_000,
        pid: 4242,
        datid: 16_385,
        datname: "appdb".to_owned(),
        relid: 16_384,
        schemaname: None,
        relname: None,
        is_autovacuum: true,
        phase: "vacuuming indexes".to_owned(),
        heap_blks_total: 10_000,
        heap_blks_scanned: 10_000,
        heap_blks_vacuumed: 4_000,
        index_vacuum_count: 1,
        max_dead_tuple_bytes: 67_108_864,
        dead_tuple_bytes: 2_500_000,
        num_dead_item_ids: 120_000,
        indexes_total: 3,
        indexes_processed: 1,
    }
}

fn v3_raw() -> ProgressVacuumV3Row {
    ProgressVacuumV3Row {
        ts: 2_000,
        pid: 4242,
        datid: 16_385,
        datname: "appdb".to_owned(),
        relid: 16_384,
        schemaname: Some("public".to_owned()),
        relname: Some("orders".to_owned()),
        is_autovacuum: true,
        phase: "vacuuming indexes".to_owned(),
        heap_blks_total: 10_000,
        heap_blks_scanned: 10_000,
        heap_blks_vacuumed: 4_000,
        index_vacuum_count: 1,
        max_dead_tuple_bytes: 67_108_864,
        dead_tuple_bytes: 2_500_000,
        num_dead_item_ids: 120_000,
        indexes_total: 3,
        indexes_processed: 1,
        delay_time: 1234.5,
    }
}

#[test]
fn version_follows_the_three_official_catalog_shapes() {
    assert_eq!(progress_vacuum_version(10), ProgressVacuumVersion::V1);
    assert_eq!(progress_vacuum_version(16), ProgressVacuumVersion::V1);
    assert_eq!(progress_vacuum_version(17), ProgressVacuumVersion::V2);
    assert_eq!(progress_vacuum_version(18), ProgressVacuumVersion::V3);
}

#[test]
fn query_includes_only_its_version_specific_columns() {
    let v1 = progress_vacuum_query(ProgressVacuumVersion::V1);
    let v2 = progress_vacuum_query(ProgressVacuumVersion::V2);
    let v3 = progress_vacuum_query(ProgressVacuumVersion::V3);
    assert!(v1.contains("max_dead_tuples"));
    assert!(!v1.contains("dead_tuple_bytes"));
    assert!(v2.contains("dead_tuple_bytes"));
    assert!(v2.contains("indexes_total"));
    assert!(!v2.contains("num_dead_tuples"));
    assert!(!v2.contains("delay_time"));
    assert!(v3.contains("delay_time"));
    for sql in [v1, v2, v3] {
        assert!(sql.contains("pg_stat_progress_vacuum"));
        assert!(sql.contains("v.datid"));
        assert!(sql.contains("pg_stat_activity"));
        assert!(sql.contains("is_autovacuum"));
        assert!(sql.contains("LEFT JOIN pg_class c ON c.oid = v.relid"));
        assert!(sql.contains("LEFT JOIN pg_namespace n ON n.oid = c.relnamespace"));
        assert!(sql.contains("kronika:"));
    }
}

#[test]
fn conversions_intern_labels_and_preserve_each_exact_shape() {
    let v1 = to_v1(&v1_raw(), fake_intern).expect("intern V1");
    assert_eq!(v1.pid, 4242);
    assert_eq!(v1.datname, fake_intern(b"appdb").unwrap());
    assert_eq!(v1.phase, fake_intern(b"scanning heap").unwrap());
    assert_eq!(v1.relname, Some(fake_intern(b"orders").unwrap()));
    assert_eq!(v1.num_dead_tuples, 120_000);

    let v2 = to_v2(&v2_raw(), fake_intern).expect("intern V2");
    assert_eq!(v2.dead_tuple_bytes, 2_500_000);
    assert_eq!(v2.indexes_processed, 1);
    assert_eq!(v2.relname, None);
    assert_eq!(v2.schemaname, None);

    let v3 = to_v3(&v3_raw(), fake_intern).expect("intern V3");
    assert_eq!(v3.dead_tuple_bytes, 2_500_000);
    assert!((v3.delay_time - 1234.5).abs() < f64::EPSILON);
    assert_eq!(v3.schemaname, Some(fake_intern(b"public").unwrap()));
}

#[test]
fn row_variants_retain_the_layout() {
    assert!(matches!(
        ProgressVacuumRow::V1(v1_raw()),
        ProgressVacuumRow::V1(_)
    ));
    assert!(matches!(
        ProgressVacuumRow::V2(v2_raw()),
        ProgressVacuumRow::V2(_)
    ));
    assert!(matches!(
        ProgressVacuumRow::V3(v3_raw()),
        ProgressVacuumRow::V3(_)
    ));
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v1(&v1_raw(), |_| Err("full")), Err("full"));
    assert_eq!(to_v2(&v2_raw(), |_| Err("full")), Err("full"));
    assert_eq!(to_v3(&v3_raw(), |_| Err("full")), Err("full"));
}
