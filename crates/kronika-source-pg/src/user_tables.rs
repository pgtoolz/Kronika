//! `pg_stat_user_tables` collection for types `1_013_005`..`1_013_008`.
//!
//! One row per table of the connected database, merging what the table did
//! (`pg_stat_user_tables`), what it cost in buffers (`pg_statio_user_tables`),
//! how large it is (`pg_class` plus the size functions) and how close its
//! relfrozenxid is to wraparound. The column set only grows: PG13 adds
//! `n_ins_since_vacuum`, PG16 the new-page updates and the last-scan
//! timestamps, PG18 the cumulative vacuum and analyze times.

use std::sync::Arc;

use kronika_registry::pg_stat_user_tables::{
    PgStatUserTablesV1, PgStatUserTablesV2, PgStatUserTablesV3, PgStatUserTablesV4,
};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::databases::Database;
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

/// The `pg_stat_user_tables` layout selected by the server major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTablesVersion {
    /// PG 10-12: type `1_013_005` (base layout).
    V1,
    /// PG 13-15: type `1_013_006` (adds `n_ins_since_vacuum`).
    V2,
    /// PG 16-17: type `1_013_007` (adds new-page updates and last-scan times).
    V3,
    /// PG 18: type `1_013_008` (adds cumulative vacuum and analyze times).
    V4,
}

/// Select the layout for a server major version.
#[must_use]
pub const fn user_tables_version(major: u32) -> UserTablesVersion {
    if major >= 18 {
        UserTablesVersion::V4
    } else if major >= 16 {
        UserTablesVersion::V3
    } else if major >= 13 {
        UserTablesVersion::V2
    } else {
        UserTablesVersion::V1
    }
}

/// The columns every layout shares.
const COMMON_COLUMNS: &str = "st.relid, \
     st.schemaname::text AS schemaname, st.relname::text AS relname, \
     CASE WHEN c.relkind = 'p' THEN NULL \
          ELSE COALESCE(NULLIF(c.reltablespace, 0), d.dattablespace) \
     END::oid AS tablespace_oid, \
     CASE WHEN c.relkind = 'p' THEN NULL ELSE ets.spcname::text END AS tablespace, \
     coalesce(st.seq_scan, 0) AS seq_scan, \
     coalesce(st.seq_tup_read, 0) AS seq_tup_read, \
     st.idx_scan, st.idx_tup_fetch, \
     coalesce(st.n_tup_ins, 0) AS n_tup_ins, \
     coalesce(st.n_tup_upd, 0) AS n_tup_upd, \
     coalesce(st.n_tup_del, 0) AS n_tup_del, \
     coalesce(st.n_tup_hot_upd, 0) AS n_tup_hot_upd, \
     coalesce(st.n_live_tup, 0) AS n_live_tup, \
     coalesce(st.n_dead_tup, 0) AS n_dead_tup, \
     coalesce(st.n_mod_since_analyze, 0) AS n_mod_since_analyze, \
     coalesce(st.vacuum_count, 0) AS vacuum_count, \
     coalesce(st.autovacuum_count, 0) AS autovacuum_count, \
     coalesce(st.analyze_count, 0) AS analyze_count, \
     coalesce(st.autoanalyze_count, 0) AS autoanalyze_count, \
     (extract(epoch from st.last_vacuum) * 1e6)::int8 AS last_vacuum_us, \
     (extract(epoch from st.last_autovacuum) * 1e6)::int8 AS last_autovacuum_us, \
     (extract(epoch from st.last_analyze) * 1e6)::int8 AS last_analyze_us, \
     (extract(epoch from st.last_autoanalyze) * 1e6)::int8 AS last_autoanalyze_us, \
     coalesce(pg_catalog.pg_relation_size(st.relid), 0) AS main_fork_bytes, \
     CASE WHEN c.reltoastrelid <> 0 \
          THEN pg_catalog.pg_total_relation_size(c.reltoastrelid) END AS toast_bytes, \
     tst.n_live_tup AS toast_n_live_tup, \
     tst.n_dead_tup AS toast_n_dead_tup, \
     (extract(epoch from tst.last_autovacuum) * 1e6)::int8 AS toast_last_autovacuum_us, \
     CASE WHEN c.relkind = 'p' THEN NULL \
          ELSE pg_catalog.age(c.relfrozenxid)::int8 END AS xid_age, \
     CASE WHEN c.relkind = 'p' THEN NULL \
          ELSE pg_catalog.mxid_age(c.relminmxid)::int8 END AS mxid_age, \
     c.reltuples::int8 AS reltuples, \
     coalesce(sio.heap_blks_read, 0) AS heap_blks_read, \
     coalesce(sio.heap_blks_hit, 0) AS heap_blks_hit, \
     sio.idx_blks_read, sio.idx_blks_hit, \
     sio.toast_blks_read, sio.toast_blks_hit, \
     sio.tidx_blks_read, sio.tidx_blks_hit, \
     (extract(epoch from pg_catalog.statement_timestamp()) * 1e6)::int8 AS ts_us";

/// The joins every layout shares.
///
/// The TOAST relation is joined through `pg_stat_all_tables` rather than the
/// `user` view: a TOAST table lives in `pg_toast`, which the `user` views leave
/// out.
const COMMON_FROM: &str = " FROM pg_catalog.pg_stat_user_tables st \
     JOIN pg_catalog.pg_class c ON c.oid = st.relid \
     JOIN pg_catalog.pg_database d ON d.datname = pg_catalog.current_database() \
     LEFT JOIN pg_catalog.pg_statio_user_tables sio ON sio.relid = st.relid \
     LEFT JOIN pg_catalog.pg_stat_all_tables tst ON tst.relid = c.reltoastrelid \
     LEFT JOIN pg_catalog.pg_tablespace ets \
       ON ets.oid = COALESCE(NULLIF(c.reltablespace, 0), d.dattablespace)";

/// The SQL for one layout.
#[must_use]
pub fn user_tables_query(version: UserTablesVersion) -> String {
    let mut columns = String::from(COMMON_COLUMNS);
    if since_v2(version) {
        columns.push_str(", coalesce(st.n_ins_since_vacuum, 0) AS n_ins_since_vacuum");
    }
    if since_v3(version) {
        columns.push_str(
            ", coalesce(st.n_tup_newpage_upd, 0) AS n_tup_newpage_upd, \
             (extract(epoch from st.last_seq_scan) * 1e6)::int8 AS last_seq_scan_us, \
             (extract(epoch from st.last_idx_scan) * 1e6)::int8 AS last_idx_scan_us",
        );
    }
    if matches!(version, UserTablesVersion::V4) {
        columns.push_str(
            ", coalesce(st.total_vacuum_time, 0) AS total_vacuum_time, \
             coalesce(st.total_autovacuum_time, 0) AS total_autovacuum_time, \
             coalesce(st.total_analyze_time, 0) AS total_analyze_time, \
             coalesce(st.total_autoanalyze_time, 0) AS total_autoanalyze_time",
        );
    }
    format!("{}{columns}{COMMON_FROM}", marked!("SELECT "))
}

/// Whether the layout carries `n_ins_since_vacuum` (PG13+).
const fn since_v2(version: UserTablesVersion) -> bool {
    matches!(
        version,
        UserTablesVersion::V2 | UserTablesVersion::V3 | UserTablesVersion::V4
    )
}

/// Whether the layout carries the PG16 columns.
const fn since_v3(version: UserTablesVersion) -> bool {
    matches!(version, UserTablesVersion::V3 | UserTablesVersion::V4)
}

/// One raw `pg_stat_user_tables` row, a version-agnostic superset.
///
/// Columns absent from the layout are `None`. See [`PgStatUserTablesV4`] for
/// column meanings.
#[derive(Debug, Clone)]
pub struct UserTablesRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Database oid of the connection that produced this row.
    pub datid: u32,
    /// Database name of the connection.
    pub datname: Arc<str>,
    /// Table oid.
    pub relid: u32,
    /// Schema name.
    pub schemaname: String,
    /// Table name.
    pub relname: String,
    /// Effective tablespace oid; `None` for a storage-less partitioned parent.
    pub tablespace_oid: Option<u32>,
    /// Effective tablespace name; `None` for a storage-less parent or missing label.
    pub tablespace: Option<String>,
    /// Sequential scans.
    pub seq_scan: i64,
    /// Live rows fetched by sequential scans.
    pub seq_tup_read: i64,
    /// Index scans; `None` when the table has no indexes.
    pub idx_scan: Option<i64>,
    /// Live rows fetched by index scans; `None` when the table has no indexes.
    pub idx_tup_fetch: Option<i64>,
    /// Rows inserted.
    pub n_tup_ins: i64,
    /// Rows updated.
    pub n_tup_upd: i64,
    /// Rows deleted.
    pub n_tup_del: i64,
    /// Rows HOT-updated.
    pub n_tup_hot_upd: i64,
    /// Rows updated to a new page (V3+).
    pub n_tup_newpage_upd: Option<i64>,
    /// Estimated live rows.
    pub n_live_tup: i64,
    /// Estimated dead rows.
    pub n_dead_tup: i64,
    /// Rows modified since the last analyze.
    pub n_mod_since_analyze: i64,
    /// Rows inserted since the last vacuum (V2+).
    pub n_ins_since_vacuum: Option<i64>,
    /// Manual vacuums.
    pub vacuum_count: i64,
    /// Autovacuums.
    pub autovacuum_count: i64,
    /// Manual analyzes.
    pub analyze_count: i64,
    /// Autoanalyzes.
    pub autoanalyze_count: i64,
    /// Last manual vacuum, unix microseconds; `None` if never.
    pub last_vacuum: Option<i64>,
    /// Last autovacuum, unix microseconds; `None` if never.
    pub last_autovacuum: Option<i64>,
    /// Last manual analyze, unix microseconds; `None` if never.
    pub last_analyze: Option<i64>,
    /// Last autoanalyze, unix microseconds; `None` if never.
    pub last_autoanalyze: Option<i64>,
    /// Last sequential scan, unix microseconds (V3+); `None` if never.
    pub last_seq_scan: Option<i64>,
    /// Last index scan, unix microseconds (V3+); `None` if never.
    pub last_idx_scan: Option<i64>,
    /// Cumulative manual-vacuum time, ms (V4+).
    pub total_vacuum_time: Option<f64>,
    /// Cumulative autovacuum time, ms (V4+).
    pub total_autovacuum_time: Option<f64>,
    /// Cumulative manual-analyze time, ms (V4+).
    pub total_analyze_time: Option<f64>,
    /// Cumulative autoanalyze time, ms (V4+).
    pub total_autoanalyze_time: Option<f64>,
    /// Main-fork size in bytes.
    pub main_fork_bytes: i64,
    /// TOAST table and its indexes in bytes; `None` when no TOAST relation.
    pub toast_bytes: Option<i64>,
    /// TOAST live tuples; `None` when no TOAST relation.
    pub toast_n_live_tup: Option<i64>,
    /// TOAST dead tuples; `None` when no TOAST relation.
    pub toast_n_dead_tup: Option<i64>,
    /// Last TOAST autovacuum, unix microseconds; `None` when never.
    pub toast_last_autovacuum: Option<i64>,
    /// Age of `relfrozenxid` in transactions; `None` for a partitioned parent.
    pub xid_age: Option<i64>,
    /// Age of `relminmxid` in multixacts; `None` for a partitioned parent.
    pub mxid_age: Option<i64>,
    /// Planner row estimate.
    pub reltuples: i64,
    /// Heap block reads.
    pub heap_blks_read: i64,
    /// Heap buffer hits.
    pub heap_blks_hit: i64,
    /// Index block reads; `None` when the table has no indexes.
    pub idx_blks_read: Option<i64>,
    /// Index buffer hits; `None` when the table has no indexes.
    pub idx_blks_hit: Option<i64>,
    /// TOAST block reads; `None` when no TOAST relation.
    pub toast_blks_read: Option<i64>,
    /// TOAST buffer hits; `None` when no TOAST relation.
    pub toast_blks_hit: Option<i64>,
    /// TOAST-index block reads; `None` when no TOAST relation.
    pub tidx_blks_read: Option<i64>,
    /// TOAST-index buffer hits; `None` when no TOAST relation.
    pub tidx_blks_hit: Option<i64>,
}

/// Build a `1_013_008` row (PG18 layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v4<E>(
    row: &UserTablesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserTablesV4, E> {
    Ok(PgStatUserTablesV4 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        seq_scan: row.seq_scan,
        seq_tup_read: row.seq_tup_read,
        idx_scan: row.idx_scan,
        idx_tup_fetch: row.idx_tup_fetch,
        n_tup_ins: row.n_tup_ins,
        n_tup_upd: row.n_tup_upd,
        n_tup_del: row.n_tup_del,
        n_tup_hot_upd: row.n_tup_hot_upd,
        n_tup_newpage_upd: row.n_tup_newpage_upd.unwrap_or(0),
        n_live_tup: row.n_live_tup,
        n_dead_tup: row.n_dead_tup,
        n_mod_since_analyze: row.n_mod_since_analyze,
        n_ins_since_vacuum: row.n_ins_since_vacuum.unwrap_or(0),
        vacuum_count: row.vacuum_count,
        autovacuum_count: row.autovacuum_count,
        analyze_count: row.analyze_count,
        autoanalyze_count: row.autoanalyze_count,
        last_vacuum: row.last_vacuum.map(Ts),
        last_autovacuum: row.last_autovacuum.map(Ts),
        last_analyze: row.last_analyze.map(Ts),
        last_autoanalyze: row.last_autoanalyze.map(Ts),
        last_seq_scan: row.last_seq_scan.map(Ts),
        last_idx_scan: row.last_idx_scan.map(Ts),
        total_vacuum_time: row.total_vacuum_time.unwrap_or(0.0),
        total_autovacuum_time: row.total_autovacuum_time.unwrap_or(0.0),
        total_analyze_time: row.total_analyze_time.unwrap_or(0.0),
        total_autoanalyze_time: row.total_autoanalyze_time.unwrap_or(0.0),
        main_fork_bytes: row.main_fork_bytes,
        toast_bytes: row.toast_bytes,
        toast_n_live_tup: row.toast_n_live_tup,
        toast_n_dead_tup: row.toast_n_dead_tup,
        toast_last_autovacuum: row.toast_last_autovacuum.map(Ts),
        xid_age: row.xid_age,
        mxid_age: row.mxid_age,
        reltuples: row.reltuples,
        heap_blks_read: row.heap_blks_read,
        heap_blks_hit: row.heap_blks_hit,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
        toast_blks_read: row.toast_blks_read,
        toast_blks_hit: row.toast_blks_hit,
        tidx_blks_read: row.tidx_blks_read,
        tidx_blks_hit: row.tidx_blks_hit,
    })
}

/// Build a `1_013_007` row (PG16-17 layout, no cumulative times).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v3<E>(
    row: &UserTablesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserTablesV3, E> {
    Ok(PgStatUserTablesV3 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        seq_scan: row.seq_scan,
        seq_tup_read: row.seq_tup_read,
        idx_scan: row.idx_scan,
        idx_tup_fetch: row.idx_tup_fetch,
        n_tup_ins: row.n_tup_ins,
        n_tup_upd: row.n_tup_upd,
        n_tup_del: row.n_tup_del,
        n_tup_hot_upd: row.n_tup_hot_upd,
        n_tup_newpage_upd: row.n_tup_newpage_upd.unwrap_or(0),
        n_live_tup: row.n_live_tup,
        n_dead_tup: row.n_dead_tup,
        n_mod_since_analyze: row.n_mod_since_analyze,
        n_ins_since_vacuum: row.n_ins_since_vacuum.unwrap_or(0),
        vacuum_count: row.vacuum_count,
        autovacuum_count: row.autovacuum_count,
        analyze_count: row.analyze_count,
        autoanalyze_count: row.autoanalyze_count,
        last_vacuum: row.last_vacuum.map(Ts),
        last_autovacuum: row.last_autovacuum.map(Ts),
        last_analyze: row.last_analyze.map(Ts),
        last_autoanalyze: row.last_autoanalyze.map(Ts),
        last_seq_scan: row.last_seq_scan.map(Ts),
        last_idx_scan: row.last_idx_scan.map(Ts),
        main_fork_bytes: row.main_fork_bytes,
        toast_bytes: row.toast_bytes,
        toast_n_live_tup: row.toast_n_live_tup,
        toast_n_dead_tup: row.toast_n_dead_tup,
        toast_last_autovacuum: row.toast_last_autovacuum.map(Ts),
        xid_age: row.xid_age,
        mxid_age: row.mxid_age,
        reltuples: row.reltuples,
        heap_blks_read: row.heap_blks_read,
        heap_blks_hit: row.heap_blks_hit,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
        toast_blks_read: row.toast_blks_read,
        toast_blks_hit: row.toast_blks_hit,
        tidx_blks_read: row.tidx_blks_read,
        tidx_blks_hit: row.tidx_blks_hit,
    })
}

/// Build a `1_013_006` row (PG13-15 layout, no PG16 columns).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v2<E>(
    row: &UserTablesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserTablesV2, E> {
    Ok(PgStatUserTablesV2 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        seq_scan: row.seq_scan,
        seq_tup_read: row.seq_tup_read,
        idx_scan: row.idx_scan,
        idx_tup_fetch: row.idx_tup_fetch,
        n_tup_ins: row.n_tup_ins,
        n_tup_upd: row.n_tup_upd,
        n_tup_del: row.n_tup_del,
        n_tup_hot_upd: row.n_tup_hot_upd,
        n_live_tup: row.n_live_tup,
        n_dead_tup: row.n_dead_tup,
        n_mod_since_analyze: row.n_mod_since_analyze,
        n_ins_since_vacuum: row.n_ins_since_vacuum.unwrap_or(0),
        vacuum_count: row.vacuum_count,
        autovacuum_count: row.autovacuum_count,
        analyze_count: row.analyze_count,
        autoanalyze_count: row.autoanalyze_count,
        last_vacuum: row.last_vacuum.map(Ts),
        last_autovacuum: row.last_autovacuum.map(Ts),
        last_analyze: row.last_analyze.map(Ts),
        last_autoanalyze: row.last_autoanalyze.map(Ts),
        main_fork_bytes: row.main_fork_bytes,
        toast_bytes: row.toast_bytes,
        toast_n_live_tup: row.toast_n_live_tup,
        toast_n_dead_tup: row.toast_n_dead_tup,
        toast_last_autovacuum: row.toast_last_autovacuum.map(Ts),
        xid_age: row.xid_age,
        mxid_age: row.mxid_age,
        reltuples: row.reltuples,
        heap_blks_read: row.heap_blks_read,
        heap_blks_hit: row.heap_blks_hit,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
        toast_blks_read: row.toast_blks_read,
        toast_blks_hit: row.toast_blks_hit,
        tidx_blks_read: row.tidx_blks_read,
        tidx_blks_hit: row.tidx_blks_hit,
    })
}

/// Build a `1_013_005` row (PG10-12 base layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v1<E>(
    row: &UserTablesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserTablesV1, E> {
    Ok(PgStatUserTablesV1 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        seq_scan: row.seq_scan,
        seq_tup_read: row.seq_tup_read,
        idx_scan: row.idx_scan,
        idx_tup_fetch: row.idx_tup_fetch,
        n_tup_ins: row.n_tup_ins,
        n_tup_upd: row.n_tup_upd,
        n_tup_del: row.n_tup_del,
        n_tup_hot_upd: row.n_tup_hot_upd,
        n_live_tup: row.n_live_tup,
        n_dead_tup: row.n_dead_tup,
        n_mod_since_analyze: row.n_mod_since_analyze,
        vacuum_count: row.vacuum_count,
        autovacuum_count: row.autovacuum_count,
        analyze_count: row.analyze_count,
        autoanalyze_count: row.autoanalyze_count,
        last_vacuum: row.last_vacuum.map(Ts),
        last_autovacuum: row.last_autovacuum.map(Ts),
        last_analyze: row.last_analyze.map(Ts),
        last_autoanalyze: row.last_autoanalyze.map(Ts),
        main_fork_bytes: row.main_fork_bytes,
        toast_bytes: row.toast_bytes,
        toast_n_live_tup: row.toast_n_live_tup,
        toast_n_dead_tup: row.toast_n_dead_tup,
        toast_last_autovacuum: row.toast_last_autovacuum.map(Ts),
        xid_age: row.xid_age,
        mxid_age: row.mxid_age,
        reltuples: row.reltuples,
        heap_blks_read: row.heap_blks_read,
        heap_blks_hit: row.heap_blks_hit,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
        toast_blks_read: row.toast_blks_read,
        toast_blks_hit: row.toast_blks_hit,
        tidx_blks_read: row.tidx_blks_read,
        tidx_blks_hit: row.tidx_blks_hit,
    })
}

/// Read a raw row, filling in the database the connection is attached to.
fn row_from_pg(
    row: query::IndexedRow<'_>,
    datid: u32,
    datname: &Arc<str>,
    version: UserTablesVersion,
) -> anyhow::Result<UserTablesRow> {
    Ok(UserTablesRow {
        ts: row.try_get("ts_us")?,
        datid,
        datname: Arc::clone(datname),
        relid: row.try_get("relid")?,
        schemaname: row.try_get("schemaname")?,
        relname: row.try_get("relname")?,
        tablespace_oid: row.try_get("tablespace_oid")?,
        tablespace: row.try_get("tablespace")?,
        seq_scan: row.try_get("seq_scan")?,
        seq_tup_read: row.try_get("seq_tup_read")?,
        idx_scan: row.try_get("idx_scan")?,
        idx_tup_fetch: row.try_get("idx_tup_fetch")?,
        n_tup_ins: row.try_get("n_tup_ins")?,
        n_tup_upd: row.try_get("n_tup_upd")?,
        n_tup_del: row.try_get("n_tup_del")?,
        n_tup_hot_upd: row.try_get("n_tup_hot_upd")?,
        n_tup_newpage_upd: since_v3(version)
            .then(|| row.try_get("n_tup_newpage_upd"))
            .transpose()?,
        n_live_tup: row.try_get("n_live_tup")?,
        n_dead_tup: row.try_get("n_dead_tup")?,
        n_mod_since_analyze: row.try_get("n_mod_since_analyze")?,
        n_ins_since_vacuum: since_v2(version)
            .then(|| row.try_get("n_ins_since_vacuum"))
            .transpose()?,
        vacuum_count: row.try_get("vacuum_count")?,
        autovacuum_count: row.try_get("autovacuum_count")?,
        analyze_count: row.try_get("analyze_count")?,
        autoanalyze_count: row.try_get("autoanalyze_count")?,
        last_vacuum: row.try_get("last_vacuum_us")?,
        last_autovacuum: row.try_get("last_autovacuum_us")?,
        last_analyze: row.try_get("last_analyze_us")?,
        last_autoanalyze: row.try_get("last_autoanalyze_us")?,
        last_seq_scan: if since_v3(version) {
            row.try_get("last_seq_scan_us")?
        } else {
            None
        },
        last_idx_scan: if since_v3(version) {
            row.try_get("last_idx_scan_us")?
        } else {
            None
        },
        total_vacuum_time: matches!(version, UserTablesVersion::V4)
            .then(|| row.try_get("total_vacuum_time"))
            .transpose()?,
        total_autovacuum_time: matches!(version, UserTablesVersion::V4)
            .then(|| row.try_get("total_autovacuum_time"))
            .transpose()?,
        total_analyze_time: matches!(version, UserTablesVersion::V4)
            .then(|| row.try_get("total_analyze_time"))
            .transpose()?,
        total_autoanalyze_time: matches!(version, UserTablesVersion::V4)
            .then(|| row.try_get("total_autoanalyze_time"))
            .transpose()?,
        main_fork_bytes: row.try_get("main_fork_bytes")?,
        toast_bytes: row.try_get("toast_bytes")?,
        toast_n_live_tup: row.try_get("toast_n_live_tup")?,
        toast_n_dead_tup: row.try_get("toast_n_dead_tup")?,
        toast_last_autovacuum: row.try_get("toast_last_autovacuum_us")?,
        xid_age: row.try_get("xid_age")?,
        mxid_age: row.try_get("mxid_age")?,
        reltuples: row.try_get("reltuples")?,
        heap_blks_read: row.try_get("heap_blks_read")?,
        heap_blks_hit: row.try_get("heap_blks_hit")?,
        idx_blks_read: row.try_get("idx_blks_read")?,
        idx_blks_hit: row.try_get("idx_blks_hit")?,
        toast_blks_read: row.try_get("toast_blks_read")?,
        toast_blks_hit: row.try_get("toast_blks_hit")?,
        tidx_blks_read: row.try_get("tidx_blks_read")?,
        tidx_blks_hit: row.try_get("tidx_blks_hit")?,
    })
}

/// Collect every table of the database `client` is attached to.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if the query fails.
pub async fn collect_user_tables<E>(
    session: Session<'_>,
    database: &Database,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<UserTablesRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = user_tables_version(major);
    let datname = Arc::<str>::from(database.name.as_str());
    query::read_batched(
        session,
        &user_tables_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| row_from_pg(row, database.oid, &datname, version),
        |_row| 0,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests;
