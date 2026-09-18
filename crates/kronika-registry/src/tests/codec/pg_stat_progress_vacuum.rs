use super::{PgStatProgressVacuumV1, PgStatProgressVacuumV2, PgStatProgressVacuumV3};
use crate::{ColumnType, Section, Semantics, StrId, Ts, TypeContract, Unit};

const COMMON: [(&str, ColumnType, bool); 8] = [
    ("ts", ColumnType::Ts, false),
    ("pid", ColumnType::I32, false),
    ("datid", ColumnType::U32, false),
    ("datname", ColumnType::StrId, false),
    ("relid", ColumnType::U32, false),
    ("schemaname", ColumnType::StrId, true),
    ("relname", ColumnType::StrId, true),
    ("is_autovacuum", ColumnType::Bool, false),
];
const AFTER_PHASE: [(&str, ColumnType, bool); 4] = [
    ("phase", ColumnType::StrId, false),
    ("heap_blks_total", ColumnType::I64, false),
    ("heap_blks_scanned", ColumnType::I64, false),
    ("heap_blks_vacuumed", ColumnType::I64, false),
];

fn assert_contract(contract: TypeContract, type_id: u32, tail: &[(&str, ColumnType, bool)]) {
    assert_eq!(contract.type_id.get(), type_id);
    assert_eq!(contract.semantics, Semantics::ConditionalFull);
    assert_eq!(contract.sort_key, ["ts", "pid"]);
    assert_eq!(contract.identity, ["pid", "datid", "relid"]);
    let expected = COMMON
        .iter()
        .chain(AFTER_PHASE.iter())
        .chain(tail)
        .copied()
        .collect::<Vec<_>>();
    let actual = contract
        .columns
        .iter()
        .map(|column| (column.name, column.ty, column.nullable))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

fn v1_row(ts: i64, pid: i32) -> PgStatProgressVacuumV1 {
    PgStatProgressVacuumV1 {
        ts: Ts(ts),
        pid,
        datid: 16_385,
        datname: StrId(7),
        relid: 16_384,
        schemaname: Some(StrId(11)),
        relname: Some(StrId(12)),
        is_autovacuum: false,
        phase: StrId(9),
        heap_blks_total: 10_000,
        heap_blks_scanned: 4_200,
        heap_blks_vacuumed: 4_000,
        index_vacuum_count: 1,
        max_dead_tuples: 291_271,
        num_dead_tuples: 120_000,
    }
}

fn v2_row(ts: i64, pid: i32) -> PgStatProgressVacuumV2 {
    PgStatProgressVacuumV2 {
        ts: Ts(ts),
        pid,
        datid: 16_385,
        datname: StrId(7),
        relid: 16_384,
        schemaname: Some(StrId(11)),
        relname: None,
        is_autovacuum: false,
        phase: StrId(9),
        heap_blks_total: 10_000,
        heap_blks_scanned: 4_200,
        heap_blks_vacuumed: 4_000,
        index_vacuum_count: 1,
        max_dead_tuple_bytes: 67_108_864,
        dead_tuple_bytes: 2_500_000,
        num_dead_item_ids: 120_000,
        indexes_total: 3,
        indexes_processed: 1,
    }
}

fn v3_row(ts: i64, pid: i32) -> PgStatProgressVacuumV3 {
    PgStatProgressVacuumV3 {
        ts: Ts(ts),
        pid,
        datid: 16_385,
        datname: StrId(7),
        relid: 16_384,
        schemaname: None,
        relname: None,
        is_autovacuum: false,
        phase: StrId(9),
        heap_blks_total: 10_000,
        heap_blks_scanned: 4_200,
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
fn contracts_match_the_three_official_view_shapes() {
    assert_contract(
        PgStatProgressVacuumV1::CONTRACT,
        1_012_004,
        &[
            ("index_vacuum_count", ColumnType::I64, false),
            ("max_dead_tuples", ColumnType::I64, false),
            ("num_dead_tuples", ColumnType::I64, false),
        ],
    );
    assert_contract(
        PgStatProgressVacuumV2::CONTRACT,
        1_012_005,
        &[
            ("index_vacuum_count", ColumnType::I64, false),
            ("max_dead_tuple_bytes", ColumnType::I64, false),
            ("dead_tuple_bytes", ColumnType::I64, false),
            ("num_dead_item_ids", ColumnType::I64, false),
            ("indexes_total", ColumnType::I64, false),
            ("indexes_processed", ColumnType::I64, false),
        ],
    );
    assert_contract(
        PgStatProgressVacuumV3::CONTRACT,
        1_012_006,
        &[
            ("index_vacuum_count", ColumnType::I64, false),
            ("max_dead_tuple_bytes", ColumnType::I64, false),
            ("dead_tuple_bytes", ColumnType::I64, false),
            ("num_dead_item_ids", ColumnType::I64, false),
            ("indexes_total", ColumnType::I64, false),
            ("indexes_processed", ColumnType::I64, false),
            ("delay_time", ColumnType::F64, false),
        ],
    );
    assert_eq!(
        PgStatProgressVacuumV1::CONTRACT
            .column("relname")
            .map(|column| column.nullable),
        Some(true)
    );
    assert_eq!(
        PgStatProgressVacuumV2::CONTRACT
            .column("dead_tuple_bytes")
            .and_then(|column| column.unit),
        Some(Unit::Bytes)
    );
    assert_eq!(
        PgStatProgressVacuumV3::CONTRACT
            .column("delay_time")
            .and_then(|column| column.unit),
        Some(Unit::Milliseconds)
    );
}

#[test]
fn each_layout_roundtrips_with_and_without_a_resolved_relation_name() {
    crate::assert_roundtrips(&[v1_row(1_000_000, 100)]);
    crate::assert_roundtrips(&[v2_row(1_000_000, 200)]);
    crate::assert_roundtrips(&[v3_row(1_000_000, 300)]);
}
