use super::PgWalStorage;
use crate::{ColumnClass, Section, Semantics, Ts, Unit};

#[test]
fn contract_and_codec_match_the_singleton_gauge() {
    let contract = PgWalStorage::CONTRACT;
    assert_eq!(contract.type_id.get(), 1_020_001);
    assert_eq!(contract.name, "pg_wal_storage");
    assert_eq!(contract.columns.len(), 2);
    assert_eq!(contract.sort_key, ["ts"]);
    assert!(contract.identity.is_empty());
    assert_eq!(contract.semantics, Semantics::SnapshotFull);
    assert_eq!(
        contract
            .column("wal_files_bytes")
            .map(|column| column.class),
        Some(ColumnClass::Gauge)
    );
    assert_eq!(
        contract
            .column("wal_files_bytes")
            .and_then(|column| column.unit),
        Some(Unit::Bytes)
    );
    assert_eq!(
        contract
            .column("wal_files_bytes")
            .map(|column| column.nullable),
        Some(false)
    );
    crate::assert_roundtrips(&[
        PgWalStorage {
            ts: Ts(1_000_000),
            wal_files_bytes: 0,
        },
        PgWalStorage {
            ts: Ts(2_000_000),
            wal_files_bytes: 33_554_432,
        },
    ]);
}
