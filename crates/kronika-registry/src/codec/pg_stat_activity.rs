//! Type `1_001_001` / `1_001_002` / `1_001_004`: `pg_stat_activity`.
//!
//! One snapshot row per backend. The view gained `leader_pid` in PG13 and
//! `query_id` in PG14, so the source maps to three layout versions.

use crate::{Section, StrId, Ts};

/// Type `1_001_004`: `pg_stat_activity` on PG 14-18 (V2 plus database and query IDs).
///
/// One row per backend in a full snapshot. Background backends (`walwriter`,
/// `checkpointer`, autovacuum, …) have no database, role, state, or running
/// query, so those columns are `None`. `ts` is one `statement_timestamp()` for
/// the whole snapshot; `backend_xid_age` / `backend_xmin_age` hold `age()` in
/// transactions, and `backend_xmin_age` is the vacuum-holdback signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_001_004,
    name = "pg_stat_activity",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct PgStatActivityV3 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Backend process id.
    #[column(l)]
    pub pid: i32,
    /// Parallel-group leader pid; `None` outside a parallel query.
    #[column(l)]
    pub leader_pid: Option<i32>,
    /// Database OID; `None` for shared/background backends.
    #[column(l)]
    pub datid: Option<u32>,
    /// Database name; `None` for background backends.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name; `None` for background backends.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Reported application name; empty string when unset.
    #[column(l)]
    pub application_name: StrId,
    /// Client host as text; empty string for a local (socket) connection.
    #[column(l)]
    pub client_addr: StrId,
    /// Backend type, e.g. `client backend`, `walwriter`.
    #[column(l)]
    pub backend_type: StrId,
    /// Backend state (`active`, `idle`, …); `None` for background backends.
    #[column(l)]
    pub state: Option<StrId>,
    /// Wait-event class; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event_type: Option<StrId>,
    /// Wait-event name; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event: Option<StrId>,
    /// Query text via the dictionary, truncated to `track_activity_query_size`;
    /// `None` for background backends.
    #[column(l)]
    pub query: Option<StrId>,
    /// Query id; `None` when `compute_query_id` is off or no statement runs.
    #[column(l)]
    pub query_id: Option<i64>,
    /// Age of the backend's xid in transactions; `None` without an assigned xid.
    #[column(g, unit = count)]
    pub backend_xid_age: Option<i64>,
    /// Age of the backend's xmin horizon; drives the vacuum-holdback signal.
    #[column(g, unit = count)]
    pub backend_xmin_age: Option<i64>,
    /// Backend start time.
    #[column(g, unit = microseconds)]
    pub backend_start: Ts,
    /// Current transaction start; `None` outside a transaction.
    #[column(g, unit = microseconds)]
    pub xact_start: Option<Ts>,
    /// Current query start; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub query_start: Option<Ts>,
    /// Last state change; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub state_change: Option<Ts>,
}

/// Type `1_001_002`: `pg_stat_activity` on PG 13 (V1 plus `leader_pid`, no
/// `query_id`). Column semantics match [`PgStatActivityV3`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_001_002,
    name = "pg_stat_activity",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct PgStatActivityV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Backend process id.
    #[column(l)]
    pub pid: i32,
    /// Parallel-group leader pid; `None` outside a parallel query.
    #[column(l)]
    pub leader_pid: Option<i32>,
    /// Database name; `None` for background backends.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name; `None` for background backends.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Reported application name; empty string when unset.
    #[column(l)]
    pub application_name: StrId,
    /// Client host as text; empty string for a local (socket) connection.
    #[column(l)]
    pub client_addr: StrId,
    /// Backend type, e.g. `client backend`, `walwriter`.
    #[column(l)]
    pub backend_type: StrId,
    /// Backend state (`active`, `idle`, …); `None` for background backends.
    #[column(l)]
    pub state: Option<StrId>,
    /// Wait-event class; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event_type: Option<StrId>,
    /// Wait-event name; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event: Option<StrId>,
    /// Query text via the dictionary, truncated to `track_activity_query_size`;
    /// `None` for background backends.
    #[column(l)]
    pub query: Option<StrId>,
    /// Age of the backend's xid in transactions; `None` without an assigned xid.
    #[column(g, unit = count)]
    pub backend_xid_age: Option<i64>,
    /// Age of the backend's xmin horizon; drives the vacuum-holdback signal.
    #[column(g, unit = count)]
    pub backend_xmin_age: Option<i64>,
    /// Backend start time.
    #[column(g, unit = microseconds)]
    pub backend_start: Ts,
    /// Current transaction start; `None` outside a transaction.
    #[column(g, unit = microseconds)]
    pub xact_start: Option<Ts>,
    /// Current query start; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub query_start: Option<Ts>,
    /// Last state change; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub state_change: Option<Ts>,
}

/// Type `1_001_001`: `pg_stat_activity` on PG 10-12 (no `leader_pid`, no
/// `query_id`). Column semantics match [`PgStatActivityV3`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_001_001,
    name = "pg_stat_activity",
    semantics = snapshot_full,
    sort_key("pid", "ts"),
    identity("pid")
)]
pub struct PgStatActivityV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Backend process id.
    #[column(l)]
    pub pid: i32,
    /// Database name; `None` for background backends.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Role name; `None` for background backends.
    #[column(l)]
    pub usename: Option<StrId>,
    /// Reported application name; empty string when unset.
    #[column(l)]
    pub application_name: StrId,
    /// Client host as text; empty string for a local (socket) connection.
    #[column(l)]
    pub client_addr: StrId,
    /// Backend type, e.g. `client backend`, `walwriter`.
    #[column(l)]
    pub backend_type: StrId,
    /// Backend state (`active`, `idle`, …); `None` for background backends.
    #[column(l)]
    pub state: Option<StrId>,
    /// Wait-event class; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event_type: Option<StrId>,
    /// Wait-event name; `None` when the backend is not waiting.
    #[column(l)]
    pub wait_event: Option<StrId>,
    /// Query text via the dictionary, truncated to `track_activity_query_size`;
    /// `None` for background backends.
    #[column(l)]
    pub query: Option<StrId>,
    /// Age of the backend's xid in transactions; `None` without an assigned xid.
    #[column(g, unit = count)]
    pub backend_xid_age: Option<i64>,
    /// Age of the backend's xmin horizon; drives the vacuum-holdback signal.
    #[column(g, unit = count)]
    pub backend_xmin_age: Option<i64>,
    /// Backend start time.
    #[column(g, unit = microseconds)]
    pub backend_start: Ts,
    /// Current transaction start; `None` outside a transaction.
    #[column(g, unit = microseconds)]
    pub xact_start: Option<Ts>,
    /// Current query start; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub query_start: Option<Ts>,
    /// Last state change; `None` for background backends.
    #[column(g, unit = microseconds)]
    pub state_change: Option<Ts>,
}

#[cfg(test)]
mod tests {
    use super::{PgStatActivityV1, PgStatActivityV2, PgStatActivityV3};
    use crate::{Section, StrId, Ts, Unit, VerifiedSection};

    fn assert_timestamp_units(c: crate::TypeContract) {
        for name in ["backend_start", "xact_start", "query_start", "state_change"] {
            assert_eq!(
                c.column(name).and_then(|column| column.unit),
                Some(Unit::Microseconds)
            );
        }
    }

    /// Client backend with every nullable field filled.
    fn v3_client(ts: i64, pid: i32) -> PgStatActivityV3 {
        PgStatActivityV3 {
            ts: Ts(ts),
            pid,
            leader_pid: None,
            datid: Some(u32::MAX),
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: Some(StrId(7)),
            wait_event: Some(StrId(8)),
            query: Some(StrId(9)),
            query_id: Some(i64::MIN),
            backend_xid_age: Some(10),
            backend_xmin_age: Some(20),
            backend_start: Ts(ts - 9_000),
            xact_start: Some(Ts(ts - 500)),
            query_start: Some(Ts(ts - 100)),
            state_change: Some(Ts(ts - 50)),
        }
    }

    /// A background worker: no database, user, state, or query.
    fn v3_background(ts: i64, pid: i32) -> PgStatActivityV3 {
        PgStatActivityV3 {
            ts: Ts(ts),
            pid,
            leader_pid: None,
            datid: None,
            datname: None,
            usename: None,
            application_name: StrId(0),
            client_addr: StrId(0),
            backend_type: StrId(11),
            state: None,
            wait_event_type: Some(StrId(12)),
            wait_event: Some(StrId(13)),
            query: None,
            query_id: None,
            backend_xid_age: None,
            backend_xmin_age: None,
            backend_start: Ts(ts - 99_000),
            xact_start: None,
            query_start: None,
            state_change: None,
        }
    }

    #[test]
    fn v3_contract_shape_matches_the_registry() {
        let c = PgStatActivityV3::CONTRACT;
        assert_eq!(c.type_id.get(), 1_001_004);
        assert_eq!(c.columns.len(), 20);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
        // `pid` and `ts` form the sort key and are never null.
        assert_eq!(c.column("ts").map(|col| col.nullable), Some(false));
        assert_eq!(c.column("pid").map(|col| col.nullable), Some(false));
        // Background backends leave these empty, so they are nullable.
        assert_eq!(c.column("datname").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("state").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("query").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("query_start").map(|col| col.nullable), Some(true));
        // Version-specific columns present on V3.
        assert_eq!(c.column("leader_pid").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("datid").map(|col| col.nullable), Some(true));
        assert_eq!(c.column("query_id").map(|col| col.nullable), Some(true));
        // backend_xmin_age is a gauge that may be absent.
        assert_eq!(
            c.column("backend_xmin_age").map(|col| col.nullable),
            Some(true)
        );
        // Always present on a client or background backend.
        assert_eq!(
            c.column("backend_type").map(|col| col.nullable),
            Some(false)
        );
        assert_timestamp_units(c);
    }

    #[test]
    fn v3_roundtrip_preserves_values_and_nulls() {
        // Encode sorts by the contract key.
        crate::assert_roundtrips(&[v3_background(1_000, 5), v3_client(2_000, 10)]);
    }

    #[test]
    fn v3_encode_sorts_by_pid_then_ts() {
        let rows = [
            v3_client(2_000, 5),
            v3_client(1_000, 20),
            v3_client(1_000, 5),
        ];
        let bytes = PgStatActivityV3::encode(&rows).expect("encode");
        let decoded =
            PgStatActivityV3::decode(VerifiedSection::for_test(bytes.into())).expect("decode");
        assert_eq!(
            decoded.iter().map(|r| (r.ts.0, r.pid)).collect::<Vec<_>>(),
            [(1_000, 5), (2_000, 5), (1_000, 20)]
        );
    }

    #[test]
    fn v3_empty_section_roundtrips() {
        let bytes = PgStatActivityV3::encode(&[]).expect("encode empty");
        assert_eq!(
            PgStatActivityV3::decode(VerifiedSection::for_test(bytes.into()))
                .expect("decode empty"),
            Vec::new()
        );
    }

    /// A client backend on the PG13 layout (no `query_id`).
    fn v2_client(ts: i64, pid: i32) -> PgStatActivityV2 {
        PgStatActivityV2 {
            ts: Ts(ts),
            pid,
            leader_pid: Some(pid - 1),
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: None,
            wait_event: None,
            query: Some(StrId(9)),
            backend_xid_age: Some(10),
            backend_xmin_age: Some(20),
            backend_start: Ts(ts - 9_000),
            xact_start: Some(Ts(ts - 500)),
            query_start: Some(Ts(ts - 100)),
            state_change: Some(Ts(ts - 50)),
        }
    }

    #[test]
    fn v2_contract_shape_has_leader_pid_without_query_id() {
        let c = PgStatActivityV2::CONTRACT;
        assert_eq!(c.type_id.get(), 1_001_002);
        assert_eq!(c.columns.len(), 18);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
        assert!(c.column("leader_pid").is_some());
        assert!(c.column("query_id").is_none());
        assert_timestamp_units(c);
    }

    #[test]
    fn v2_roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[v2_client(1_000, 5), v2_client(2_000, 10)]);
    }

    /// A client backend on the PG10-12 layout (no `leader_pid`, no `query_id`).
    fn v1_client(ts: i64, pid: i32) -> PgStatActivityV1 {
        PgStatActivityV1 {
            ts: Ts(ts),
            pid,
            datname: Some(StrId(1)),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: Some(StrId(7)),
            wait_event: Some(StrId(8)),
            query: Some(StrId(9)),
            backend_xid_age: None,
            backend_xmin_age: Some(20),
            backend_start: Ts(ts - 9_000),
            xact_start: None,
            query_start: Some(Ts(ts - 100)),
            state_change: Some(Ts(ts - 50)),
        }
    }

    #[test]
    fn v1_contract_shape_has_neither_leader_pid_nor_query_id() {
        let c = PgStatActivityV1::CONTRACT;
        assert_eq!(c.type_id.get(), 1_001_001);
        assert_eq!(c.columns.len(), 17);
        assert_eq!(c.sort_key, ["pid", "ts"]);
        assert_eq!(c.identity, ["pid"]);
        assert!(c.column("leader_pid").is_none());
        assert!(c.column("query_id").is_none());
        assert_timestamp_units(c);
    }

    #[test]
    fn v1_roundtrip_preserves_values_and_nulls() {
        crate::assert_roundtrips(&[v1_client(1_000, 5), v1_client(2_000, 10)]);
    }
}
