//! Collects `pg_stat_activity` rows for types `1_001_001`, `1_001_002`, and
//! `1_001_004`.
//!
//! Collection returns owned raw rows. The collector interns strings when it
//! writes the segment dictionary, so this crate does not depend on the writer.

use kronika_registry::pg_stat_activity::{PgStatActivityV1, PgStatActivityV2, PgStatActivityV3};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

/// The `pg_stat_activity` layout selected by the server major version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityVersion {
    /// PG 10-12: type `1_001_001` (no `leader_pid`, no `query_id`).
    V1,
    /// PG 13: type `1_001_002` (adds `leader_pid`).
    V2,
    /// PG 14-18: type `1_001_004` (adds `datid` and `query_id`).
    V3,
}

/// Select the `pg_stat_activity` schema variant for a server major version.
///
/// `leader_pid` arrived in PG13 and `query_id` in PG14; below 13 is V1, 13 is
/// V2, 14 and above is V3.
#[must_use]
pub const fn activity_version(major: u32) -> ActivityVersion {
    if major >= 14 {
        ActivityVersion::V3
    } else if major == 13 {
        ActivityVersion::V2
    } else {
        ActivityVersion::V1
    }
}

/// SQL for one `pg_stat_activity` schema variant.
///
/// Each query carries the kronika marker and selects only the columns that
/// version stores. Timestamps come back as unix microseconds,
/// `backend_xid`/`backend_xmin` as `age()` in transactions, and `ts` as one
/// `statement_timestamp()` for the whole snapshot.
#[must_use]
pub const fn activity_query(version: ActivityVersion) -> &'static str {
    match version {
        ActivityVersion::V1 => marked!(
            "SELECT pid, datname::text AS datname, usename::text AS usename, \
             coalesce(application_name, '') AS application_name, \
             coalesce(host(client_addr), '') AS client_addr, \
             backend_type, state, wait_event_type, wait_event, \
             left(query, 65536) AS query, \
             age(backend_xid)::int8 AS backend_xid_age, \
             age(backend_xmin)::int8 AS backend_xmin_age, \
             (extract(epoch from backend_start) * 1e6)::int8 AS backend_start_us, \
             (extract(epoch from xact_start) * 1e6)::int8 AS xact_start_us, \
             (extract(epoch from query_start) * 1e6)::int8 AS query_start_us, \
             (extract(epoch from state_change) * 1e6)::int8 AS state_change_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_catalog.pg_stat_activity \
             WHERE pid <> pg_catalog.pg_backend_pid() \
               AND application_name IS DISTINCT FROM current_setting('application_name') \
             ORDER BY pid"
        ),
        ActivityVersion::V2 => marked!(
            "SELECT pid, leader_pid, datname::text AS datname, usename::text AS usename, \
             coalesce(application_name, '') AS application_name, \
             coalesce(host(client_addr), '') AS client_addr, \
             backend_type, state, wait_event_type, wait_event, \
             left(query, 65536) AS query, \
             age(backend_xid)::int8 AS backend_xid_age, \
             age(backend_xmin)::int8 AS backend_xmin_age, \
             (extract(epoch from backend_start) * 1e6)::int8 AS backend_start_us, \
             (extract(epoch from xact_start) * 1e6)::int8 AS xact_start_us, \
             (extract(epoch from query_start) * 1e6)::int8 AS query_start_us, \
             (extract(epoch from state_change) * 1e6)::int8 AS state_change_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_catalog.pg_stat_activity \
             WHERE pid <> pg_catalog.pg_backend_pid() \
               AND application_name IS DISTINCT FROM current_setting('application_name') \
             ORDER BY pid"
        ),
        ActivityVersion::V3 => marked!(
            "SELECT pid, leader_pid, datid, datname::text AS datname, usename::text AS usename, \
             coalesce(application_name, '') AS application_name, \
             coalesce(host(client_addr), '') AS client_addr, \
             backend_type, state, wait_event_type, wait_event, \
             left(query, 65536) AS query, query_id, \
             age(backend_xid)::int8 AS backend_xid_age, \
             age(backend_xmin)::int8 AS backend_xmin_age, \
             (extract(epoch from backend_start) * 1e6)::int8 AS backend_start_us, \
             (extract(epoch from xact_start) * 1e6)::int8 AS xact_start_us, \
             (extract(epoch from query_start) * 1e6)::int8 AS query_start_us, \
             (extract(epoch from state_change) * 1e6)::int8 AS state_change_us, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_catalog.pg_stat_activity \
             WHERE pid <> pg_catalog.pg_backend_pid() \
               AND application_name IS DISTINCT FROM current_setting('application_name') \
             ORDER BY pid"
        ),
    }
}

/// Raw `pg_stat_activity` row before string interning.
///
/// Strings are owned; the caller interns them into the segment dictionary.
/// Columns not selected by a version-specific query are `None`.
#[derive(Debug, Clone)]
pub struct ActivityRow {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Backend process id.
    pub pid: i32,
    /// Parallel-group leader pid.
    pub leader_pid: Option<i32>,
    /// Database OID; absent for shared/background backends and pre-PG14 layouts.
    pub datid: Option<u32>,
    /// Database name.
    pub datname: Option<String>,
    /// Role name.
    pub usename: Option<String>,
    /// Application name (empty string when unset).
    pub application_name: String,
    /// Client host as text (empty string for a local connection).
    pub client_addr: String,
    /// Backend type.
    pub backend_type: String,
    /// Backend state.
    pub state: Option<String>,
    /// Wait-event class.
    pub wait_event_type: Option<String>,
    /// Wait-event name.
    pub wait_event: Option<String>,
    /// Current query text.
    pub query: Option<String>,
    /// Query id.
    pub query_id: Option<i64>,
    /// Age of the backend's xid in transactions.
    pub backend_xid_age: Option<i64>,
    /// Age of the backend's xmin horizon.
    pub backend_xmin_age: Option<i64>,
    /// Backend start time, unix microseconds.
    pub backend_start: i64,
    /// Current transaction start, unix microseconds.
    pub xact_start: Option<i64>,
    /// Current query start, unix microseconds.
    pub query_start: Option<i64>,
    /// Last state change, unix microseconds.
    pub state_change: Option<i64>,
}

/// Build a `1_001_004` row, interning strings through `intern`.
///
/// # Errors
/// Returns the interner's error if any string cannot be interned.
pub fn to_v3<E>(
    row: &ActivityRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatActivityV3, E> {
    Ok(PgStatActivityV3 {
        ts: Ts(row.ts),
        pid: row.pid,
        leader_pid: row.leader_pid,
        datid: row.datid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        application_name: intern(row.application_name.as_bytes())?,
        client_addr: intern(row.client_addr.as_bytes())?,
        backend_type: intern(row.backend_type.as_bytes())?,
        state: opt(&mut intern, row.state.as_deref())?,
        wait_event_type: opt(&mut intern, row.wait_event_type.as_deref())?,
        wait_event: opt(&mut intern, row.wait_event.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        query_id: row.query_id,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: Ts(row.backend_start),
        xact_start: row.xact_start.map(Ts),
        query_start: row.query_start.map(Ts),
        state_change: row.state_change.map(Ts),
    })
}

/// Build a `1_001_002` row (PG13 layout, no `query_id`).
///
/// # Errors
/// Returns the interner's error if any string cannot be interned.
pub fn to_v2<E>(
    row: &ActivityRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatActivityV2, E> {
    Ok(PgStatActivityV2 {
        ts: Ts(row.ts),
        pid: row.pid,
        leader_pid: row.leader_pid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        application_name: intern(row.application_name.as_bytes())?,
        client_addr: intern(row.client_addr.as_bytes())?,
        backend_type: intern(row.backend_type.as_bytes())?,
        state: opt(&mut intern, row.state.as_deref())?,
        wait_event_type: opt(&mut intern, row.wait_event_type.as_deref())?,
        wait_event: opt(&mut intern, row.wait_event.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: Ts(row.backend_start),
        xact_start: row.xact_start.map(Ts),
        query_start: row.query_start.map(Ts),
        state_change: row.state_change.map(Ts),
    })
}

/// Build a `1_001_001` row (PG10-12 layout, no `leader_pid`, no `query_id`).
///
/// # Errors
/// Returns the interner's error if any string cannot be interned.
pub fn to_v1<E>(
    row: &ActivityRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatActivityV1, E> {
    Ok(PgStatActivityV1 {
        ts: Ts(row.ts),
        pid: row.pid,
        datname: opt(&mut intern, row.datname.as_deref())?,
        usename: opt(&mut intern, row.usename.as_deref())?,
        application_name: intern(row.application_name.as_bytes())?,
        client_addr: intern(row.client_addr.as_bytes())?,
        backend_type: intern(row.backend_type.as_bytes())?,
        state: opt(&mut intern, row.state.as_deref())?,
        wait_event_type: opt(&mut intern, row.wait_event_type.as_deref())?,
        wait_event: opt(&mut intern, row.wait_event.as_deref())?,
        query: opt(&mut intern, row.query.as_deref())?,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: Ts(row.backend_start),
        xact_start: row.xact_start.map(Ts),
        query_start: row.query_start.map(Ts),
        state_change: row.state_change.map(Ts),
    })
}

/// Read a raw row from a result row using the version's column set.
fn row_from_pg(
    row: query::IndexedRow<'_>,
    version: ActivityVersion,
) -> anyhow::Result<ActivityRow> {
    Ok(ActivityRow {
        ts: row.try_get("ts_us")?,
        pid: row.try_get("pid")?,
        leader_pid: match version {
            ActivityVersion::V1 => None,
            ActivityVersion::V2 | ActivityVersion::V3 => row.try_get("leader_pid")?,
        },
        datid: match version {
            ActivityVersion::V1 | ActivityVersion::V2 => None,
            ActivityVersion::V3 => row.try_get("datid")?,
        },
        datname: row.try_get("datname")?,
        usename: row.try_get("usename")?,
        application_name: row.try_get("application_name")?,
        client_addr: row.try_get("client_addr")?,
        backend_type: row.try_get("backend_type")?,
        state: row.try_get("state")?,
        wait_event_type: row.try_get("wait_event_type")?,
        wait_event: row.try_get("wait_event")?,
        query: row.try_get("query")?,
        query_id: match version {
            ActivityVersion::V1 | ActivityVersion::V2 => None,
            ActivityVersion::V3 => row.try_get("query_id")?,
        },
        backend_xid_age: row.try_get("backend_xid_age")?,
        backend_xmin_age: row.try_get("backend_xmin_age")?,
        backend_start: row.try_get("backend_start_us")?,
        xact_start: row.try_get("xact_start_us")?,
        query_start: row.try_get("query_start_us")?,
        state_change: row.try_get("state_change_us")?,
    })
}

/// Stream the complete `pg_stat_activity` snapshot in bounded batches.
///
/// # Errors
/// Returns the `PostgreSQL` stream error or the batch sink error.
pub async fn collect_activity<E>(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<ActivityRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = activity_version(major);
    query::read_batched(
        session,
        activity_query(version),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| row_from_pg(row, version),
        |_row| 0,
        sink,
    )
    .await
}

#[cfg(test)]
#[path = "tests/activity.rs"]
mod tests;
