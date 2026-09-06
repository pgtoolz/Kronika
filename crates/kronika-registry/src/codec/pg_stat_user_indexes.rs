//! Type `1_014_003`..`1_014_004`: `pg_stat_user_indexes`.
//!
//! Per-index statistics, one row per selected index per database. In PG 10-18
//! the column set grows once: `last_idx_scan` arrives in PG16. PG17 and PG18 add
//! nothing to `pg_stat_all_indexes`, so the source maps those catalog layouts to
//! two layout versions.
//!
//! Each layout merges `pg_statio_user_indexes` (the buffer-I/O counters), the
//! `pg_index` flags (`indisunique`/`indisprimary`/`indisvalid`/`indisexclusion`/
//! `indisready`), the access method name from `pg_am`, and `pg_get_indexdef` into
//! the same row. `indexdef` can be `None` if the index is dropped concurrently.

use crate::{Section, StrId, Ts};

/// Type `1_014_004`: `pg_stat_user_indexes` on PG 16-18 (V1 plus `last_idx_scan`).
///
/// One row per selected index per database. `last_idx_scan` is `None` when the
/// index has never been scanned. Every column is an integer, `StrId`, or `bool`,
/// so the layout derives `Eq`.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_014_004,
    name = "pg_stat_user_indexes",
    semantics = snapshot_full,
    sort_key("datid", "indexrelid", "ts"),
    identity("datid", "indexrelid")
)]
pub struct PgStatUserIndexesV2 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Index oid.
    #[column(l)]
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Index name.
    #[column(l)]
    pub indexrelname: StrId,
    /// Effective tablespace oid.
    #[column(l)]
    pub tablespace_oid: u32,
    /// Effective tablespace name; `None` when catalog label resolution is missing.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Index scans.
    #[column(c, unit = count)]
    pub idx_scan: i64,
    /// Index entries returned by scans.
    #[column(c, unit = count)]
    pub idx_tup_read: i64,
    /// Live table rows fetched by simple index scans using this index.
    #[column(c, unit = count)]
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes (`pg_relation_size(indexrelid)`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// Last index scan (PG16+); `None` if never.
    #[column(g, unit = microseconds)]
    pub last_idx_scan: Option<Ts>,
    /// Whether the index enforces uniqueness.
    #[column(l)]
    pub indisunique: bool,
    /// Whether the index is a primary key.
    #[column(l)]
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    #[column(l)]
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    #[column(l)]
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    #[column(l)]
    pub indisready: bool,
    /// Access method name (`btree`, `hash`, `gin`, ...).
    #[column(l)]
    pub amname: StrId,
    /// `pg_get_indexdef` reconstruction; `None` after a concurrent index drop.
    #[column(l)]
    pub indexdef: Option<StrId>,
    /// Shared-buffer misses for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_hit: i64,
}

/// Type `1_014_003`: `pg_stat_user_indexes` on PG 10-15 (base layout, no
/// `last_idx_scan`). Column meanings match [`PgStatUserIndexesV2`] for fields
/// present in this layout.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_014_003,
    name = "pg_stat_user_indexes",
    semantics = snapshot_full,
    sort_key("datid", "indexrelid", "ts"),
    identity("datid", "indexrelid")
)]
pub struct PgStatUserIndexesV1 {
    /// Snapshot time, unix microseconds (per-database `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Database oid of the connection that produced this row.
    #[column(l)]
    pub datid: u32,
    /// Database name of the connection.
    #[column(l)]
    pub datname: StrId,
    /// Index oid.
    #[column(l)]
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    #[column(l)]
    pub relid: u32,
    /// Schema name.
    #[column(l)]
    pub schemaname: StrId,
    /// Table name.
    #[column(l)]
    pub relname: StrId,
    /// Index name.
    #[column(l)]
    pub indexrelname: StrId,
    /// Effective tablespace oid.
    #[column(l)]
    pub tablespace_oid: u32,
    /// Effective tablespace name; `None` when catalog label resolution is missing.
    #[column(l)]
    pub tablespace: Option<StrId>,
    /// Index scans.
    #[column(c, unit = count)]
    pub idx_scan: i64,
    /// Index entries returned by scans.
    #[column(c, unit = count)]
    pub idx_tup_read: i64,
    /// Live table rows fetched by simple index scans using this index.
    #[column(c, unit = count)]
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes (`pg_relation_size(indexrelid)`).
    #[column(g, unit = bytes)]
    pub main_fork_bytes: i64,
    /// Whether the index enforces uniqueness.
    #[column(l)]
    pub indisunique: bool,
    /// Whether the index is a primary key.
    #[column(l)]
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    #[column(l)]
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    #[column(l)]
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    #[column(l)]
    pub indisready: bool,
    /// Access method name (`btree`, `hash`, `gin`, ...).
    #[column(l)]
    pub amname: StrId,
    /// `pg_get_indexdef` reconstruction; `None` after a concurrent index drop.
    #[column(l)]
    pub indexdef: Option<StrId>,
    /// Shared-buffer misses for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    #[column(c, unit = count)]
    pub idx_blks_hit: i64,
}

#[cfg(test)]
mod tests {
    use super::{PgStatUserIndexesV1, PgStatUserIndexesV2};
    use crate::{Section, StrId, Ts, Unit, VerifiedSection};

    fn v2_row(ts: i64, datid: u32, indexrelid: u32) -> PgStatUserIndexesV2 {
        PgStatUserIndexesV2 {
            ts: Ts(ts),
            datid,
            datname: StrId(u64::from(datid) | 1),
            indexrelid,
            relid: indexrelid - 1,
            schemaname: StrId(2),
            relname: StrId(3),
            indexrelname: StrId(u64::from(indexrelid) | 1),
            tablespace_oid: 1_663,
            tablespace: Some(StrId(4)),
            idx_scan: 120,
            idx_tup_read: 3_400,
            idx_tup_fetch: 3_000,
            main_fork_bytes: 16_384,
            last_idx_scan: Some(Ts(ts - 1)),
            indisunique: true,
            indisprimary: true,
            indisvalid: true,
            indisexclusion: false,
            indisready: true,
            amname: StrId(5),
            indexdef: Some(StrId(6)),
            idx_blks_read: 40,
            idx_blks_hit: 9_000,
        }
    }

    #[test]
    fn v2_contract_shape() {
        let c = PgStatUserIndexesV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_014_004);
        assert_eq!(c.columns.len(), 24);
        assert_eq!(c.sort_key, ["datid", "indexrelid", "ts"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("indexrelid").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("tablespace_oid").map(|col| col.nullable),
            Some(false)
        );
        assert_eq!(c.column("tablespace").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("idx_scan").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("last_idx_scan").map(|col| col.nullable),
            Some(true)
        );
        assert!(c.column("main_fork_bytes").is_some());
        assert!(c.column("size_bytes").is_none());
        assert!(c.column("indisunique").is_some());
        assert_eq!(
            c.column("indisexclusion").map(|col| col.nullable),
            Some(false)
        );
        assert_eq!(c.column("indisready").map(|col| col.nullable), Some(false));
        assert!(c.column("amname").is_some());
        assert_eq!(c.column("indexdef").map(|col| col.nullable), Some(true));
        assert_eq!(
            c.column("last_idx_scan").and_then(|col| col.unit),
            Some(Unit::Microseconds)
        );
    }

    #[test]
    fn v1_is_base_layout() {
        let c = PgStatUserIndexesV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_014_003);
        assert_eq!(c.columns.len(), 23);
        assert_eq!(c.sort_key, ["datid", "indexrelid", "ts"]);
        assert!(c.column("last_idx_scan").is_none());
        assert!(c.column("main_fork_bytes").is_some());
        assert!(c.column("idx_blks_hit").is_some());
        assert!(c.column("indisexclusion").is_some());
        assert!(c.column("indisready").is_some());
        assert_eq!(c.column("indexdef").map(|col| col.nullable), Some(true));
    }

    #[test]
    fn v2_roundtrip() {
        let mut never_scanned = v2_row(5, 5, 16_384);
        never_scanned.last_idx_scan = None;
        never_scanned.indexdef = None;
        crate::assert_roundtrips(&[
            never_scanned,
            v2_row(1_000, 5, 16_384),
            v2_row(1_000, 5, 16_385),
        ]);
    }

    #[test]
    fn v2_encode_sorts_by_datid_indexrelid_ts() {
        let bytes = PgStatUserIndexesV2::encode(&[
            v2_row(1_000, 9, 16_385),
            v2_row(1_000, 1, 16_390),
            v2_row(1_000, 1, 16_384),
        ])
        .expect("encode");
        let decoded =
            PgStatUserIndexesV2::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded
                .iter()
                .map(|r| (r.datid, r.indexrelid))
                .collect::<Vec<_>>(),
            [(1, 16_384), (1, 16_390), (9, 16_385)]
        );
    }
}
