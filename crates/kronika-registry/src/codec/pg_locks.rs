//! Type `1_011_001` / `1_011_002`: direct `pg_locks` wait graph.
//!
//! Each waiter and each positive blocker PID appears once. Direct edges are in
//! `blocked_by`; blocker-only rows have an empty list. Prepared transactions
//! remain the special blocker PID `0` but do not get their own row.
//!
//! The section splits into two layout versions because `waitstart` was added to
//! `pg_locks` in PG 14. `PgLocksV2` (PG 14-18) includes `waitstart`;
//! `PgLocksV1` (PG 10-13) is byte-identical minus that trailing field.

use crate::{Section, StrId, Ts};

/// Type `1_011_002`: `pg_locks` waits on PG 14-18 (`PgLocksV1` plus `waitstart`).
///
/// One row per involved backend; `blocked_by` holds the deduped
/// `pg_blocking_pids` edges (`0` = prepared-xact holder).
#[derive(Debug, Clone, PartialEq, Eq, Section)]
#[section(
    id = 1_011_002,
    name = "pg_locks",
    semantics = conditional_full,
    sort_key("pid"),
    identity("pid")
)]
pub struct PgLocksV2 {
    /// Snapshot time, unix microseconds (server `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Backend process id.
    #[column(l)]
    pub pid: i32,
    /// Deduped `pg_blocking_pids(pid)`; empty for roots; may contain `0`.
    #[column(l)]
    pub blocked_by: Vec<i32>,
    /// Database oid of the backend.
    #[column(l)]
    pub datid: u32,
    /// Database name of the backend.
    #[column(l)]
    pub datname: StrId,
    /// Login role; `None` for some background backends.
    #[column(l)]
    pub usename: Option<StrId>,
    /// `application_name`.
    #[column(l)]
    pub application_name: StrId,
    /// Client address as text; empty = local.
    #[column(l)]
    pub client_addr: StrId,
    /// `backend_type`.
    #[column(l)]
    pub backend_type: StrId,
    /// Session state; `None` for some background backends.
    #[column(l)]
    pub state: Option<StrId>,
    /// Wait event type; `None` for non-waiting roots.
    #[column(l)]
    pub wait_event_type: Option<StrId>,
    /// Wait event name.
    #[column(l)]
    pub wait_event: Option<StrId>,
    /// Current query (dictionary, truncated in SQL).
    #[column(l)]
    pub query: StrId,
    /// `age(backend_xid)`; `None` without an assigned xid.
    #[column(g, unit = count)]
    pub backend_xid_age: Option<i64>,
    /// `age(backend_xmin)`; vacuum-horizon hold.
    #[column(g, unit = count)]
    pub backend_xmin_age: Option<i64>,
    /// Backend start, unix microseconds.
    #[column(g, unit = microseconds)]
    pub backend_start: Option<Ts>,
    /// Transaction start; `None` outside a transaction.
    #[column(g, unit = microseconds)]
    pub xact_start: Option<Ts>,
    /// Current statement start.
    #[column(g, unit = microseconds)]
    pub query_start: Option<Ts>,
    /// Last state change.
    #[column(g, unit = microseconds)]
    pub state_change: Option<Ts>,
    /// Awaited lock type; `None` for non-waiting roots.
    #[column(l)]
    pub lock_locktype: Option<StrId>,
    /// Awaited lock mode.
    #[column(l)]
    pub lock_mode: Option<StrId>,
    /// Database oid from the awaited `pg_locks` row.
    #[column(l)]
    pub lock_database: Option<u32>,
    /// Relation oid of the awaited lock (relation/page/tuple/extend).
    #[column(l)]
    pub lock_relation: Option<u32>,
    /// Relation name, resolved only for the connected database.
    #[column(l)]
    pub lock_relname: Option<StrId>,
    /// Page number of a page/tuple lock target.
    #[column(g, unit = count)]
    pub lock_page: Option<i32>,
    /// Tuple offset of a tuple lock target.
    #[column(g, unit = count)]
    pub lock_tuple: Option<i16>,
    /// Virtual transaction id for `virtualxid` locks.
    #[column(l)]
    pub lock_virtualxid: Option<StrId>,
    /// Transaction id being awaited (row-lock pattern), raw xid.
    #[column(l)]
    pub lock_transactionid: Option<i64>,
    /// Class oid for object locks.
    #[column(l)]
    pub lock_classid: Option<u32>,
    /// Object oid for object locks.
    #[column(l)]
    pub lock_objid: Option<u32>,
    /// Object sub-id for object locks.
    #[column(l)]
    pub lock_objsubid: Option<i16>,
    /// Human-readable target, best effort.
    #[column(l)]
    pub lock_target: Option<StrId>,
    /// Lock-wait start (PG14+); nullable even while waiting.
    #[column(g, unit = microseconds)]
    pub waitstart: Option<Ts>,
}

/// Type `1_011_001`: `pg_locks` waits on PG 10-13 (base layout, no
/// `waitstart`). Column meanings match [`PgLocksV2`] for fields present in
/// this layout.
#[derive(Debug, Clone, PartialEq, Eq, Section)]
#[section(
    id = 1_011_001,
    name = "pg_locks",
    semantics = conditional_full,
    sort_key("pid"),
    identity("pid")
)]
pub struct PgLocksV1 {
    /// Snapshot time, unix microseconds (server `statement_timestamp()`).
    #[column(t)]
    pub ts: Ts,
    /// Backend process id.
    #[column(l)]
    pub pid: i32,
    /// Deduped `pg_blocking_pids(pid)`; empty for roots; may contain `0`.
    #[column(l)]
    pub blocked_by: Vec<i32>,
    /// Database oid of the backend.
    #[column(l)]
    pub datid: u32,
    /// Database name of the backend.
    #[column(l)]
    pub datname: StrId,
    /// Login role; `None` for some background backends.
    #[column(l)]
    pub usename: Option<StrId>,
    /// `application_name`.
    #[column(l)]
    pub application_name: StrId,
    /// Client address as text; empty = local.
    #[column(l)]
    pub client_addr: StrId,
    /// `backend_type`.
    #[column(l)]
    pub backend_type: StrId,
    /// Session state; `None` for some background backends.
    #[column(l)]
    pub state: Option<StrId>,
    /// Wait event type; `None` for non-waiting roots.
    #[column(l)]
    pub wait_event_type: Option<StrId>,
    /// Wait event name.
    #[column(l)]
    pub wait_event: Option<StrId>,
    /// Current query (dictionary, truncated in SQL).
    #[column(l)]
    pub query: StrId,
    /// `age(backend_xid)`; `None` without an assigned xid.
    #[column(g, unit = count)]
    pub backend_xid_age: Option<i64>,
    /// `age(backend_xmin)`; vacuum-horizon hold.
    #[column(g, unit = count)]
    pub backend_xmin_age: Option<i64>,
    /// Backend start, unix microseconds.
    #[column(g, unit = microseconds)]
    pub backend_start: Option<Ts>,
    /// Transaction start; `None` outside a transaction.
    #[column(g, unit = microseconds)]
    pub xact_start: Option<Ts>,
    /// Current statement start.
    #[column(g, unit = microseconds)]
    pub query_start: Option<Ts>,
    /// Last state change.
    #[column(g, unit = microseconds)]
    pub state_change: Option<Ts>,
    /// Awaited lock type; `None` for non-waiting roots.
    #[column(l)]
    pub lock_locktype: Option<StrId>,
    /// Awaited lock mode.
    #[column(l)]
    pub lock_mode: Option<StrId>,
    /// Database oid from the awaited `pg_locks` row.
    #[column(l)]
    pub lock_database: Option<u32>,
    /// Relation oid of the awaited lock (relation/page/tuple/extend).
    #[column(l)]
    pub lock_relation: Option<u32>,
    /// Relation name, resolved only for the connected database.
    #[column(l)]
    pub lock_relname: Option<StrId>,
    /// Page number of a page/tuple lock target.
    #[column(g, unit = count)]
    pub lock_page: Option<i32>,
    /// Tuple offset of a tuple lock target.
    #[column(g, unit = count)]
    pub lock_tuple: Option<i16>,
    /// Virtual transaction id for `virtualxid` locks.
    #[column(l)]
    pub lock_virtualxid: Option<StrId>,
    /// Transaction id being awaited (row-lock pattern), raw xid.
    #[column(l)]
    pub lock_transactionid: Option<i64>,
    /// Class oid for object locks.
    #[column(l)]
    pub lock_classid: Option<u32>,
    /// Object oid for object locks.
    #[column(l)]
    pub lock_objid: Option<u32>,
    /// Object sub-id for object locks.
    #[column(l)]
    pub lock_objsubid: Option<i16>,
    /// Human-readable target, best effort.
    #[column(l)]
    pub lock_target: Option<StrId>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_locks.rs"]
mod tests;
