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
mod tests {
    use super::{PgLocksV1, PgLocksV2};
    use crate::{Section, StrId, Ts, VerifiedSection};

    /// A blocker-only backend: no waiter columns populated.
    fn v2_row(ts: i64, pid: i32) -> PgLocksV2 {
        PgLocksV2 {
            ts: Ts(ts),
            pid,
            blocked_by: vec![],
            datid: 16_384,
            datname: StrId(1),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: None,
            wait_event: None,
            query: StrId(7),
            backend_xid_age: None,
            backend_xmin_age: None,
            backend_start: Some(Ts(ts - 60_000_000)),
            xact_start: Some(Ts(ts - 5_000_000)),
            query_start: Some(Ts(ts - 1_000_000)),
            state_change: Some(Ts(ts - 1_000_000)),
            lock_locktype: None,
            lock_mode: None,
            lock_database: None,
            lock_relation: None,
            lock_relname: None,
            lock_page: None,
            lock_tuple: None,
            lock_virtualxid: None,
            lock_transactionid: None,
            lock_classid: None,
            lock_objid: None,
            lock_objsubid: None,
            lock_target: None,
            waitstart: None,
        }
    }

    /// A blocker-only backend on the PG 10-13 layout.
    fn v1_row(ts: i64, pid: i32) -> PgLocksV1 {
        PgLocksV1 {
            ts: Ts(ts),
            pid,
            blocked_by: vec![],
            datid: 16_384,
            datname: StrId(1),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: None,
            wait_event: None,
            query: StrId(7),
            backend_xid_age: None,
            backend_xmin_age: None,
            backend_start: Some(Ts(ts - 60_000_000)),
            xact_start: Some(Ts(ts - 5_000_000)),
            query_start: Some(Ts(ts - 1_000_000)),
            state_change: Some(Ts(ts - 1_000_000)),
            lock_locktype: None,
            lock_mode: None,
            lock_database: None,
            lock_relation: None,
            lock_relname: None,
            lock_page: None,
            lock_tuple: None,
            lock_virtualxid: None,
            lock_transactionid: None,
            lock_classid: None,
            lock_objid: None,
            lock_objsubid: None,
            lock_target: None,
        }
    }

    #[test]
    fn v2_contract_shape() {
        let c = PgLocksV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_011_002);
        assert_eq!(c.columns.len(), 33);
        assert_eq!(c.sort_key, ["pid"]);
        assert_eq!(c.identity, ["pid"]);
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(
            c.column("blocked_by").map(|col| col.ty),
            Some(crate::ColumnType::ListI32)
        );
        assert!(c.column("waitstart").is_some());
        assert_eq!(
            c.column("wait_event_type").map(|col| col.nullable),
            Some(true)
        );
        assert_eq!(
            c.column("lock_page").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I32, true))
        );
        assert_eq!(
            c.column("lock_virtualxid")
                .map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::StrId, true))
        );
        assert_eq!(
            c.column("lock_tuple").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I16, true))
        );
        assert_eq!(
            c.column("lock_objsubid").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I16, true))
        );
        assert!(c.column("root_pid").is_none());
        assert!(c.column("depth").is_none());
        assert!(c.column("lock_granted").is_none());
        assert!(c.column("lock_fastpath").is_none());
    }

    #[test]
    fn v1_drops_waitstart() {
        let c = PgLocksV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_011_001);
        assert_eq!(c.columns.len(), 32);
        assert_eq!(c.sort_key, ["pid"]);
        assert_eq!(c.identity, ["pid"]);
        assert!(c.column("waitstart").is_none());
        assert!(c.column("blocked_by").is_some());
        assert_eq!(
            c.column("lock_page").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I32, true))
        );
        assert_eq!(
            c.column("lock_virtualxid")
                .map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::StrId, true))
        );
        assert_eq!(
            c.column("lock_tuple").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I16, true))
        );
        assert_eq!(
            c.column("lock_objsubid").map(|col| (col.ty, col.nullable)),
            Some((crate::ColumnType::I16, true))
        );
        assert!(c.column("root_pid").is_none());
        assert!(c.column("depth").is_none());
        assert!(c.column("lock_granted").is_none());
        assert!(c.column("lock_fastpath").is_none());
    }

    #[test]
    fn v2_roundtrip() {
        let root = v2_row(1_000_000, 10);
        let mut waiter = v2_row(1_000_000, 20);
        waiter.blocked_by = vec![10, 0]; // multi-element with 0
        waiter.wait_event_type = Some(StrId(8));
        waiter.wait_event = Some(StrId(9));
        waiter.lock_locktype = Some(StrId(10));
        waiter.lock_mode = Some(StrId(11));
        waiter.lock_database = Some(16_384);
        waiter.lock_relation = Some(12_345);
        waiter.lock_relname = Some(StrId(12));
        waiter.lock_page = Some(42);
        waiter.lock_tuple = Some(7);
        waiter.lock_virtualxid = Some(StrId(14));
        waiter.lock_transactionid = Some(999_999);
        waiter.lock_classid = Some(1_250);
        waiter.lock_objid = Some(12_345);
        waiter.lock_objsubid = Some(2);
        waiter.lock_target = Some(StrId(13));
        waiter.waitstart = Some(Ts(999_000));
        crate::assert_roundtrips(&[root, waiter]);
    }

    #[test]
    fn v2_roundtrip_empty_and_zero_blocked_by() {
        // Root has empty blocked_by; isolated has vec![0].
        let root = v2_row(2_000_000, 5);
        let mut solo = v2_row(2_000_000, 7);
        solo.blocked_by = vec![0];
        crate::assert_roundtrips(&[root, solo]);
    }

    #[test]
    fn v2_encode_sorts_by_pid() {
        let rows = [
            v2_row(1_000_000, 30),
            v2_row(1_000_000, 10),
            v2_row(1_000_000, 5),
        ];
        let bytes = PgLocksV2::encode(&rows).expect("encode");
        let decoded = PgLocksV2::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded.iter().map(|r| r.pid).collect::<Vec<_>>(),
            [5, 10, 30]
        );
    }

    #[test]
    fn v2_nullable_awaited_lock_columns_roundtrip() {
        let with_lock = {
            let mut r = v2_row(1_000_000, 99);
            r.waitstart = Some(Ts(500_000));
            r.lock_locktype = Some(StrId(20));
            r.lock_mode = Some(StrId(21));
            r.lock_database = Some(16_384);
            r.lock_relation = Some(54_321);
            r.lock_page = Some(3);
            r.lock_tuple = Some(11);
            r.lock_virtualxid = Some(StrId(22));
            r.lock_transactionid = Some(42);
            r.lock_classid = Some(1_250);
            r.lock_objid = Some(54_321);
            r.lock_objsubid = Some(4);
            r
        };
        let without_lock = v2_row(1_000_000, 100);

        let bytes = PgLocksV2::encode(&[with_lock.clone(), without_lock.clone()]).expect("encode");
        let decoded = PgLocksV2::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(decoded[0], with_lock);
        assert_eq!(decoded[1], without_lock);
        assert_eq!(decoded[0].waitstart, Some(Ts(500_000)));
        assert_eq!(decoded[1].waitstart, None);
        assert_eq!(decoded[0].lock_relation, Some(54_321));
        assert_eq!(decoded[1].lock_relation, None);
        assert_eq!(decoded[0].lock_database, Some(16_384));
        assert_eq!(decoded[1].lock_database, None);
        assert_eq!(decoded[0].lock_page, Some(3));
        assert_eq!(decoded[1].lock_page, None);
        assert_eq!(decoded[0].lock_tuple, Some(11));
        assert_eq!(decoded[1].lock_tuple, None);
        assert_eq!(decoded[0].lock_virtualxid, Some(StrId(22)));
        assert_eq!(decoded[1].lock_virtualxid, None);
        assert_eq!(decoded[0].lock_classid, Some(1_250));
        assert_eq!(decoded[1].lock_classid, None);
        assert_eq!(decoded[0].lock_objid, Some(54_321));
        assert_eq!(decoded[1].lock_objid, None);
        assert_eq!(decoded[0].lock_objsubid, Some(4));
        assert_eq!(decoded[1].lock_objsubid, None);
    }

    #[test]
    fn v1_golden_root_waiter_and_zero_blocker() {
        // Root: not blocked, so every awaited-lock column is None.
        let root = v1_row(3_000_000, 100);
        // Waiter blocked by the root, with a fully populated awaited lock.
        let mut waiter = v1_row(3_000_000, 200);
        waiter.blocked_by = vec![100];
        waiter.wait_event_type = Some(StrId(8));
        waiter.wait_event = Some(StrId(9));
        waiter.lock_locktype = Some(StrId(10));
        waiter.lock_mode = Some(StrId(11));
        waiter.lock_database = Some(16_384);
        waiter.lock_relation = Some(12_345);
        waiter.lock_relname = Some(StrId(12));
        waiter.lock_page = Some(42);
        waiter.lock_tuple = Some(7);
        waiter.lock_virtualxid = Some(StrId(14));
        waiter.lock_transactionid = Some(999_999);
        waiter.lock_classid = Some(1_250);
        waiter.lock_objid = Some(12_345);
        waiter.lock_objsubid = Some(2);
        waiter.lock_target = Some(StrId(13));
        // Blocked behind a prepared-transaction holder: pg_blocking_pids yields 0.
        let mut orphan = v1_row(3_000_000, 300);
        orphan.blocked_by = vec![0];

        // Encode order matches the pid sort so decode preserves it.
        let rows = [root, waiter, orphan];
        let bytes = PgLocksV1::encode(&rows).expect("encode");
        let decoded = PgLocksV1::decode(VerifiedSection::for_test(bytes.into())).expect("decode");

        assert_eq!(decoded.as_slice(), &rows, "PG10-13 locks roundtrip");
        assert_eq!(
            decoded[0].blocked_by,
            Vec::<i32>::new(),
            "root has no edges"
        );
        assert!(
            decoded[0].lock_locktype.is_none(),
            "an unblocked root has no awaited lock"
        );
        assert_eq!(
            decoded[1].blocked_by,
            vec![100],
            "waiter points at the root"
        );
        assert_eq!(
            decoded[1].lock_locktype,
            Some(StrId(10)),
            "the waiter's awaited lock survives the roundtrip"
        );
        assert_eq!(
            decoded[2].blocked_by,
            vec![0],
            "a prepared-xact holder is recorded as pid 0"
        );
    }
}
