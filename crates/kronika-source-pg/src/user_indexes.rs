//! `pg_stat_user_indexes` collection for types `1_014_003`..`1_014_004`.
//!
//! One row per index of the connected database. The scan counters come from
//! `pg_stat_user_indexes`, the buffer counters from `pg_statio_user_indexes`,
//! and the shape of the index from `pg_index`, `pg_am` and `pg_get_indexdef`,
//! so a row says both how the index was used and what it is. `last_idx_scan`
//! arrives in PG16 and selects the layout.

use std::sync::Arc;

use kronika_registry::pg_stat_user_indexes::{PgStatUserIndexesV1, PgStatUserIndexesV2};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::databases::Database;
use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

/// The `pg_stat_user_indexes` layout selected by the server major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIndexesVersion {
    /// PG 10-15: type `1_014_003` (base layout).
    V1,
    /// PG 16-18: type `1_014_004` (adds `last_idx_scan`).
    V2,
}

/// Select the layout for a server major version.
#[must_use]
pub const fn user_indexes_version(major: u32) -> UserIndexesVersion {
    if major >= 16 {
        UserIndexesVersion::V2
    } else {
        UserIndexesVersion::V1
    }
}

/// The columns every layout shares, as one SQL fragment.
const COMMON_COLUMNS: &str = "si.indexrelid, si.relid, \
     si.schemaname::text AS schemaname, si.relname::text AS relname, \
     si.indexrelname::text AS indexrelname, \
     COALESCE(NULLIF(c.reltablespace, 0), d.dattablespace)::oid AS tablespace_oid, \
     ets.spcname::text AS tablespace, \
     coalesce(si.idx_scan, 0) AS idx_scan, \
     coalesce(si.idx_tup_read, 0) AS idx_tup_read, \
     coalesce(si.idx_tup_fetch, 0) AS idx_tup_fetch, \
     coalesce(pg_catalog.pg_relation_size(si.indexrelid), 0) AS main_fork_bytes, \
     i.indisunique, i.indisprimary, i.indisvalid, i.indisexclusion, i.indisready, \
     am.amname::text AS amname, \
     pg_catalog.left(pg_catalog.pg_get_indexdef(si.indexrelid), 65536) AS indexdef, \
     coalesce(sio.idx_blks_read, 0) AS idx_blks_read, \
     coalesce(sio.idx_blks_hit, 0) AS idx_blks_hit, \
     (extract(epoch from pg_catalog.statement_timestamp()) * 1e6)::int8 AS ts_us";

/// The joins every layout shares.
///
/// `pg_tablespace` is joined twice because `reltablespace` is `0` for a
/// relation in the database default, which is the tablespace to report.
const COMMON_FROM: &str = " FROM pg_catalog.pg_stat_user_indexes si \
     JOIN pg_catalog.pg_statio_user_indexes sio ON sio.indexrelid = si.indexrelid \
     JOIN pg_catalog.pg_class c ON c.oid = si.indexrelid \
     JOIN pg_catalog.pg_am am ON am.oid = c.relam \
     JOIN pg_catalog.pg_index i ON i.indexrelid = si.indexrelid \
     JOIN pg_catalog.pg_database d ON d.datname = pg_catalog.current_database() \
     LEFT JOIN pg_catalog.pg_tablespace ets \
       ON ets.oid = COALESCE(NULLIF(c.reltablespace, 0), d.dattablespace)";

/// The SQL for one layout.
#[must_use]
pub fn user_indexes_query(version: UserIndexesVersion) -> String {
    let extra = match version {
        UserIndexesVersion::V1 => "",
        UserIndexesVersion::V2 => {
            ", (extract(epoch from si.last_idx_scan) * 1e6)::int8 AS last_idx_scan_us"
        }
    };
    format!("{}{COMMON_COLUMNS}{extra}{COMMON_FROM}", marked!("SELECT "))
}

/// One raw `pg_stat_user_indexes` row, a version-agnostic superset.
///
/// Strings are owned; the caller interns them. See [`PgStatUserIndexesV2`] for
/// column meanings.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent pg_index flag column, not interdependent state"
)]
#[derive(Debug, Clone)]
pub struct UserIndexesRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Database oid of the connection that produced this row.
    pub datid: u32,
    /// Database name of the connection.
    pub datname: Arc<str>,
    /// Index oid.
    pub indexrelid: u32,
    /// Table oid the index belongs to.
    pub relid: u32,
    /// Schema name.
    pub schemaname: String,
    /// Table name.
    pub relname: String,
    /// Index name.
    pub indexrelname: String,
    /// Effective tablespace oid.
    pub tablespace_oid: u32,
    /// Effective tablespace name; `None` when catalog label resolution is missing.
    pub tablespace: Option<String>,
    /// Index scans.
    pub idx_scan: i64,
    /// Index entries returned by scans.
    pub idx_tup_read: i64,
    /// Live table rows fetched through this index.
    pub idx_tup_fetch: i64,
    /// Main-fork size in bytes.
    pub main_fork_bytes: i64,
    /// Last index scan, unix microseconds (V2); `None` if never or unknown.
    pub last_idx_scan: Option<i64>,
    /// Whether the index enforces uniqueness.
    pub indisunique: bool,
    /// Whether the index is a primary key.
    pub indisprimary: bool,
    /// Whether the index is valid for queries.
    pub indisvalid: bool,
    /// Whether the index enforces an exclusion constraint.
    pub indisexclusion: bool,
    /// Whether the index is ready for inserts.
    pub indisready: bool,
    /// Access method name.
    pub amname: String,
    /// Reconstructed definition; `None` if `pg_get_indexdef` saw a concurrent drop.
    pub indexdef: Option<String>,
    /// Shared-buffer misses for index blocks.
    pub idx_blks_read: i64,
    /// Shared-buffer hits for index blocks.
    pub idx_blks_hit: i64,
}

/// Build a `1_014_004` row (PG16+ layout), interning every string.
///
/// # Errors
/// Returns the interner's error.
pub fn to_v2<E>(
    row: &UserIndexesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserIndexesV2, E> {
    Ok(PgStatUserIndexesV2 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        indexrelid: row.indexrelid,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        indexrelname: intern(row.indexrelname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        idx_scan: row.idx_scan,
        idx_tup_read: row.idx_tup_read,
        idx_tup_fetch: row.idx_tup_fetch,
        main_fork_bytes: row.main_fork_bytes,
        last_idx_scan: row.last_idx_scan.map(Ts),
        indisunique: row.indisunique,
        indisprimary: row.indisprimary,
        indisvalid: row.indisvalid,
        indisexclusion: row.indisexclusion,
        indisready: row.indisready,
        amname: intern(row.amname.as_bytes())?,
        indexdef: opt(&mut intern, row.indexdef.as_deref())?,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
    })
}

/// Build a `1_014_003` row (PG10-15 base layout).
///
/// # Errors
/// Returns the interner's error.
pub fn to_v1<E>(
    row: &UserIndexesRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatUserIndexesV1, E> {
    Ok(PgStatUserIndexesV1 {
        ts: Ts(row.ts),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        indexrelid: row.indexrelid,
        relid: row.relid,
        schemaname: intern(row.schemaname.as_bytes())?,
        relname: intern(row.relname.as_bytes())?,
        indexrelname: intern(row.indexrelname.as_bytes())?,
        tablespace_oid: row.tablespace_oid,
        tablespace: opt(&mut intern, row.tablespace.as_deref())?,
        idx_scan: row.idx_scan,
        idx_tup_read: row.idx_tup_read,
        idx_tup_fetch: row.idx_tup_fetch,
        main_fork_bytes: row.main_fork_bytes,
        indisunique: row.indisunique,
        indisprimary: row.indisprimary,
        indisvalid: row.indisvalid,
        indisexclusion: row.indisexclusion,
        indisready: row.indisready,
        amname: intern(row.amname.as_bytes())?,
        indexdef: opt(&mut intern, row.indexdef.as_deref())?,
        idx_blks_read: row.idx_blks_read,
        idx_blks_hit: row.idx_blks_hit,
    })
}

/// Read a raw row, filling in the database the connection is attached to.
fn row_from_pg(
    row: query::IndexedRow<'_>,
    datid: u32,
    datname: &Arc<str>,
    version: UserIndexesVersion,
) -> anyhow::Result<UserIndexesRow> {
    Ok(UserIndexesRow {
        ts: row.try_get("ts_us")?,
        datid,
        datname: Arc::clone(datname),
        indexrelid: row.try_get("indexrelid")?,
        relid: row.try_get("relid")?,
        schemaname: row.try_get("schemaname")?,
        relname: row.try_get("relname")?,
        indexrelname: row.try_get("indexrelname")?,
        tablespace_oid: row.try_get("tablespace_oid")?,
        tablespace: row.try_get("tablespace")?,
        idx_scan: row.try_get("idx_scan")?,
        idx_tup_read: row.try_get("idx_tup_read")?,
        idx_tup_fetch: row.try_get("idx_tup_fetch")?,
        main_fork_bytes: row.try_get("main_fork_bytes")?,
        last_idx_scan: match version {
            UserIndexesVersion::V1 => None,
            UserIndexesVersion::V2 => row.try_get("last_idx_scan_us")?,
        },
        indisunique: row.try_get("indisunique")?,
        indisprimary: row.try_get("indisprimary")?,
        indisvalid: row.try_get("indisvalid")?,
        indisexclusion: row.try_get("indisexclusion")?,
        indisready: row.try_get("indisready")?,
        amname: row.try_get("amname")?,
        indexdef: row.try_get("indexdef")?,
        idx_blks_read: row.try_get("idx_blks_read")?,
        idx_blks_hit: row.try_get("idx_blks_hit")?,
    })
}

/// Collect every index of the database `client` is attached to.
///
/// # Errors
/// Returns the [`tokio_postgres::Error`] if the query fails.
pub async fn collect_user_indexes<E>(
    session: Session<'_>,
    database: &Database,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<UserIndexesRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = user_indexes_version(major);
    let datname = Arc::<str>::from(database.name.as_str());
    query::read_batched(
        session,
        &user_indexes_query(version),
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
