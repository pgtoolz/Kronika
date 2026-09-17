use super::wal_storage_query;

#[test]
fn query_is_available_only_for_postgresql_10_through_18() {
    assert_eq!(wal_storage_query(9), None);
    for major in 10..=18 {
        assert!(wal_storage_query(major).is_some(), "PostgreSQL {major}");
    }
    assert_eq!(wal_storage_query(19), None);
}

#[test]
fn query_sums_exactly_the_visible_waldir_file_sizes() {
    let sql = wal_storage_query(18).expect("PostgreSQL 18 is supported");
    assert!(sql.contains("COALESCE(pg_catalog.sum(w.size), 0::numeric)::int8 AS wal_files_bytes"));
    assert!(sql.contains("FROM pg_catalog.pg_ls_waldir() AS w"));
    assert!(sql.contains("pg_catalog.statement_timestamp()"));
    assert!(sql.contains("kronika:"));
    assert!(!sql.contains("archive_status"));
    assert!(!sql.contains("w.name"));
    assert!(!sql.contains(" WHERE "));
}
