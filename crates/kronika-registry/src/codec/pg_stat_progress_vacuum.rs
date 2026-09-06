//! Types `1_012_004` through `1_012_006`: `pg_stat_progress_vacuum`.
//!
//! One row per backend running `VACUUM`; absence means no active rows. The
//! view changed incompatibly in PG17 and PG18, so each exact server shape has
//! its own codec. `schemaname`/`relname` are resolved from `pg_class` at
//! collection time and absent when the relation belongs to a database the
//! collector's connection cannot see or was dropped since; `relid` stays the
//! identity either way.

use crate::{Section, StrId, Ts};

/// Type `1_012_004`: `pg_stat_progress_vacuum`, PG10-16 layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_012_004,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple capacity.
    #[column(g, unit = count)]
    pub max_dead_tuples: i64,
    /// Dead tuples collected in the current cycle.
    #[column(g, unit = count)]
    pub num_dead_tuples: i64,
}

/// Type `1_012_005`: `pg_stat_progress_vacuum`, PG17 layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_012_005,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    #[column(g, unit = bytes)]
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    #[column(g, unit = bytes)]
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    #[column(g, unit = count)]
    pub num_dead_item_ids: i64,
    /// Indexes to process in this cycle.
    #[column(g, unit = count)]
    pub indexes_total: i64,
    /// Indexes processed in this cycle.
    #[column(g, unit = count)]
    pub indexes_processed: i64,
}

/// Type `1_012_006`: `pg_stat_progress_vacuum`, PG18+ layout.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_012_006,
    name = "pg_stat_progress_vacuum",
    semantics = conditional_full,
    sort_key("ts", "pid"),
    identity("pid", "datid", "relid")
)]
pub struct PgStatProgressVacuumV3 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Process id of the backend running this vacuum.
    #[column(l)]
    pub pid: i32,
    /// OID of the database being vacuumed.
    #[column(l)]
    pub datid: u32,
    /// Database being vacuumed, interned in the segment dictionary.
    #[column(l)]
    pub datname: StrId,
    /// OID of the table being vacuumed.
    #[column(l)]
    pub relid: u32,
    /// Schema of the table, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub schemaname: Option<StrId>,
    /// Table name, resolved from `pg_class`; absent when it cannot be.
    #[column(l)]
    pub relname: Option<StrId>,
    /// Whether the backend is an autovacuum worker.
    #[column(l)]
    pub is_autovacuum: bool,
    /// Current vacuum phase, such as `scanning heap`, interned.
    #[column(l)]
    pub phase: StrId,
    /// Heap blocks in the table at scan start.
    #[column(g, unit = count)]
    pub heap_blks_total: i64,
    /// Heap blocks scanned so far.
    #[column(g, unit = count)]
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed so far.
    #[column(g, unit = count)]
    pub heap_blks_vacuumed: i64,
    /// Completed index-vacuum cycles.
    #[column(g, unit = count)]
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    #[column(g, unit = bytes)]
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    #[column(g, unit = bytes)]
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    #[column(g, unit = count)]
    pub num_dead_item_ids: i64,
    /// Indexes to process in this cycle.
    #[column(g, unit = count)]
    pub indexes_total: i64,
    /// Indexes processed in this cycle.
    #[column(g, unit = count)]
    pub indexes_processed: i64,
    /// Cost-delay sleep time, milliseconds.
    #[column(g, unit = milliseconds)]
    pub delay_time: f64,
}

#[cfg(test)]
mod tests {
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
}
