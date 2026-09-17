use super::{WalVersion, wal_query, wal_version};

#[test]
fn version_appears_at_pg14_and_changes_at_pg18() {
    assert_eq!(wal_version(10), None);
    assert_eq!(wal_version(13), None);
    assert_eq!(wal_version(14), Some(WalVersion::V1));
    assert_eq!(wal_version(17), Some(WalVersion::V1));
    assert_eq!(wal_version(18), Some(WalVersion::V2));
}

#[test]
fn query_includes_version_specific_columns() {
    assert!(wal_query(WalVersion::V1).contains("wal_write_time"));
    assert!(wal_query(WalVersion::V1).contains("wal_sync"));
    assert!(!wal_query(WalVersion::V2).contains("wal_write"));
    assert!(!wal_query(WalVersion::V2).contains("wal_sync"));
    for version in [WalVersion::V1, WalVersion::V2] {
        assert!(wal_query(version).contains("pg_stat_wal"));
        assert!(wal_query(version).contains("kronika:"));
        assert!(wal_query(version).contains("wal_bytes::int8 AS wal_bytes"));
    }
}
