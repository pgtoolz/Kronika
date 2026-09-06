//! Direct `pg_locks` wait-graph collection for types `1_011_001` / `1_011_002`.
//!
//! The query returns one row per backend in a blocking component, not one row
//! per held lock. Its size is therefore bounded by the server's backend count.

use kronika_registry::pg_locks::{PgLocksV1, PgLocksV2};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

const MARKER: &str = marked!();

/// Layout selected by the `PostgreSQL` major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocksVersion {
    /// `PostgreSQL` 10-13, before `pg_locks.waitstart`.
    V1,
    /// `PostgreSQL` 14-18, with `pg_locks.waitstart`.
    V2,
}

/// Select the lock layout for a supported server.
#[must_use]
pub const fn locks_version(major: u32) -> LocksVersion {
    if major >= 14 {
        LocksVersion::V2
    } else {
        LocksVersion::V1
    }
}

/// Build the direct wait-graph query for one layout.
#[must_use]
pub fn locks_query(version: LocksVersion) -> String {
    let waitstart = match version {
        LocksVersion::V1 => "",
        LocksVersion::V2 => ", (extract(epoch from w.waitstart) * 1e6)::int8 AS waitstart_us",
    };
    format!(
        "{MARKER}WITH raw_waiters AS (\
           SELECT DISTINCT ON (l.pid) l.* FROM pg_catalog.pg_locks l \
           WHERE NOT l.granted AND l.pid IS NOT NULL \
             AND l.pid <> pg_catalog.pg_backend_pid() \
             AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_stat_activity own \
               WHERE own.pid = l.pid AND own.application_name IS NOT DISTINCT FROM \
                 current_setting('application_name')) \
           ORDER BY l.pid, l.locktype, l.database, l.relation, l.page, l.tuple, \
                    l.virtualxid::text, l.transactionid::text, \
                    l.classid, l.objid, l.objsubid\
         ), waiters AS (\
           SELECT l.*, ARRAY(\
             SELECT DISTINCT b.pid \
             FROM unnest(pg_catalog.pg_blocking_pids(l.pid)) AS b(pid) \
             WHERE b.pid <> pg_catalog.pg_backend_pid() \
               AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_stat_activity own \
                 WHERE own.pid = b.pid AND own.application_name IS NOT DISTINCT FROM \
                   current_setting('application_name')) \
             ORDER BY b.pid\
           )::int4[] AS blocked_by FROM raw_waiters l\
         ), involved(pid) AS (\
           SELECT pid FROM waiters UNION \
           SELECT b.pid FROM waiters w \
           CROSS JOIN LATERAL unnest(w.blocked_by) AS b(pid) WHERE b.pid > 0\
         ) \
         SELECT (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us, \
         i.pid, coalesce(w.blocked_by, ARRAY[]::int4[]) AS blocked_by, \
         coalesce(a.datid, 0::oid) AS datid, coalesce(a.datname::text, '') AS datname, \
         a.usename::text AS usename, coalesce(a.application_name, '') AS application_name, \
         coalesce(a.client_addr::text, '') AS client_addr, \
         coalesce(a.backend_type, '') AS backend_type, a.state, a.wait_event_type, a.wait_event, \
         left(coalesce(a.query, ''), 65536) AS query, \
         age(a.backend_xid)::int8 AS backend_xid_age, \
         age(a.backend_xmin)::int8 AS backend_xmin_age, \
         (extract(epoch from a.backend_start) * 1e6)::int8 AS backend_start_us, \
         (extract(epoch from a.xact_start) * 1e6)::int8 AS xact_start_us, \
         (extract(epoch from a.query_start) * 1e6)::int8 AS query_start_us, \
         (extract(epoch from a.state_change) * 1e6)::int8 AS state_change_us, \
         w.locktype AS lock_locktype, w.mode AS lock_mode, \
         w.database AS lock_database, w.relation AS lock_relation, c.relname::text AS lock_relname, \
         w.page AS lock_page, w.tuple AS lock_tuple, w.virtualxid::text AS lock_virtualxid, \
         w.transactionid::text::int8 AS lock_transactionid, w.classid AS lock_classid, \
         w.objid AS lock_objid, w.objsubid AS lock_objsubid, \
         CASE WHEN c.oid IS NOT NULL THEN c.relname::text \
              WHEN w.transactionid IS NOT NULL THEN 'transaction ' || w.transactionid::text \
              WHEN w.virtualxid IS NOT NULL THEN 'virtualxid ' || w.virtualxid \
              WHEN w.classid IS NOT NULL THEN 'object ' || w.classid::text || '/' || w.objid::text \
              ELSE w.locktype END AS lock_target{waitstart} \
         FROM involved i LEFT JOIN pg_catalog.pg_stat_activity a ON a.pid = i.pid \
         LEFT JOIN waiters w ON w.pid = i.pid \
         LEFT JOIN pg_catalog.pg_class c ON c.oid = w.relation \
           AND w.database = (SELECT oid FROM pg_catalog.pg_database \
                             WHERE datname = pg_catalog.current_database()) \
         ORDER BY i.pid"
    )
}

/// One direct wait-graph row before string interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRow {
    ts: i64,
    pid: i32,
    blocked_by: Vec<i32>,
    datid: u32,
    datname: String,
    usename: Option<String>,
    application_name: String,
    client_addr: String,
    backend_type: String,
    state: Option<String>,
    wait_event_type: Option<String>,
    wait_event: Option<String>,
    query: String,
    backend_xid_age: Option<i64>,
    backend_xmin_age: Option<i64>,
    backend_start: Option<i64>,
    xact_start: Option<i64>,
    query_start: Option<i64>,
    state_change: Option<i64>,
    lock_locktype: Option<String>,
    lock_mode: Option<String>,
    lock_database: Option<u32>,
    lock_relation: Option<u32>,
    lock_relname: Option<String>,
    lock_page: Option<i32>,
    lock_tuple: Option<i16>,
    lock_virtualxid: Option<String>,
    lock_transactionid: Option<i64>,
    lock_classid: Option<u32>,
    lock_objid: Option<u32>,
    lock_objsubid: Option<i16>,
    lock_target: Option<String>,
    waitstart: Option<i64>,
}

const fn blocked_by_logical_bytes(row: &LockRow) -> usize {
    row.blocked_by.len().saturating_mul(size_of::<i32>())
}

/// Build a `PostgreSQL` 10-13 row, interning labels.
///
/// # Errors
///
/// Returns the interner error.
pub fn to_v1<E>(
    row: &LockRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgLocksV1, E> {
    Ok(PgLocksV1 {
        ts: Ts(row.ts),
        pid: row.pid,
        blocked_by: row.blocked_by.clone(),
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        application_name: intern(row.application_name.as_bytes())?,
        client_addr: intern(row.client_addr.as_bytes())?,
        backend_type: intern(row.backend_type.as_bytes())?,
        state: opt(&mut intern, row.state.as_deref())?,
        wait_event_type: opt(&mut intern, row.wait_event_type.as_deref())?,
        wait_event: opt(&mut intern, row.wait_event.as_deref())?,
        query: intern(row.query.as_bytes())?,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: row.backend_start.map(Ts),
        xact_start: row.xact_start.map(Ts),
        query_start: row.query_start.map(Ts),
        state_change: row.state_change.map(Ts),
        lock_locktype: opt(&mut intern, row.lock_locktype.as_deref())?,
        lock_mode: opt(&mut intern, row.lock_mode.as_deref())?,
        lock_database: row.lock_database,
        lock_relation: row.lock_relation,
        lock_relname: opt(&mut intern, row.lock_relname.as_deref())?,
        lock_page: row.lock_page,
        lock_tuple: row.lock_tuple,
        lock_virtualxid: opt(&mut intern, row.lock_virtualxid.as_deref())?,
        lock_transactionid: row.lock_transactionid,
        lock_classid: row.lock_classid,
        lock_objid: row.lock_objid,
        lock_objsubid: row.lock_objsubid,
        lock_target: opt(&mut intern, row.lock_target.as_deref())?,
    })
}

/// Build a `PostgreSQL` 14-18 row, interning labels.
///
/// # Errors
///
/// Returns the interner error.
pub fn to_v2<E>(
    row: &LockRow,
    intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgLocksV2, E> {
    let base = to_v1(row, intern)?;
    Ok(PgLocksV2 {
        ts: base.ts,
        pid: base.pid,
        blocked_by: base.blocked_by,
        datid: base.datid,
        datname: base.datname,
        usename: base.usename,
        application_name: base.application_name,
        client_addr: base.client_addr,
        backend_type: base.backend_type,
        state: base.state,
        wait_event_type: base.wait_event_type,
        wait_event: base.wait_event,
        query: base.query,
        backend_xid_age: base.backend_xid_age,
        backend_xmin_age: base.backend_xmin_age,
        backend_start: base.backend_start,
        xact_start: base.xact_start,
        query_start: base.query_start,
        state_change: base.state_change,
        lock_locktype: base.lock_locktype,
        lock_mode: base.lock_mode,
        lock_database: base.lock_database,
        lock_relation: base.lock_relation,
        lock_relname: base.lock_relname,
        lock_page: base.lock_page,
        lock_tuple: base.lock_tuple,
        lock_virtualxid: base.lock_virtualxid,
        lock_transactionid: base.lock_transactionid,
        lock_classid: base.lock_classid,
        lock_objid: base.lock_objid,
        lock_objsubid: base.lock_objsubid,
        lock_target: base.lock_target,
        waitstart: row.waitstart.map(Ts),
    })
}

fn from_pg(
    row: query::IndexedRow<'_>,
    version: LocksVersion,
) -> Result<LockRow, tokio_postgres::Error> {
    Ok(LockRow {
        ts: row.try_get("ts_us")?,
        pid: row.try_get("pid")?,
        blocked_by: row.try_get("blocked_by")?,
        datid: row.try_get("datid")?,
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        application_name: row.try_get("application_name")?,
        client_addr: row.try_get("client_addr")?,
        backend_type: row.try_get("backend_type")?,
        state: row.try_get("state")?,
        wait_event_type: row.try_get("wait_event_type")?,
        wait_event: row.try_get("wait_event")?,
        query: row.try_get("query")?,
        backend_xid_age: row.try_get("backend_xid_age")?,
        backend_xmin_age: row.try_get("backend_xmin_age")?,
        backend_start: row.try_get("backend_start_us")?,
        xact_start: row.try_get("xact_start_us")?,
        query_start: row.try_get("query_start_us")?,
        state_change: row.try_get("state_change_us")?,
        lock_locktype: row.try_get("lock_locktype")?,
        lock_mode: row.try_get("lock_mode")?,
        lock_database: row.try_get("lock_database")?,
        lock_relation: row.try_get("lock_relation")?,
        lock_relname: row.try_get("lock_relname")?,
        lock_page: row.try_get("lock_page")?,
        lock_tuple: row.try_get("lock_tuple")?,
        lock_virtualxid: row.try_get("lock_virtualxid")?,
        lock_transactionid: row.try_get("lock_transactionid")?,
        lock_classid: row.try_get("lock_classid")?,
        lock_objid: row.try_get("lock_objid")?,
        lock_objsubid: row.try_get("lock_objsubid")?,
        lock_target: row.try_get("lock_target")?,
        waitstart: if matches!(version, LocksVersion::V2) {
            row.try_get("waitstart_us")?
        } else {
            None
        },
    })
}

/// Stream one wait graph into bounded batches using one unnamed statement.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect_locks<E>(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<LockRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = locks_version(major);
    let sql = locks_query(version);
    query::read_batched(
        session,
        &sql,
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| from_pg(row, version).map_err(anyhow::Error::new),
        blocked_by_logical_bytes,
        sink,
    )
    .await
}

#[cfg(test)]
mod tests;
