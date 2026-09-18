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
#[path = "../tests/codec/pg_stat_activity.rs"]
mod tests;
