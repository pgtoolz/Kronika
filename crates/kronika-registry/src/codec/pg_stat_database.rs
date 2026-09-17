//! Type `1_005_001`..`1_005_004`: `pg_stat_database`.
//!
//! Per-database counters. In PG 10-18 the column set only grows:
//! `checksum_failures`/`checksum_last_failure` arrive in PG12, session
//! statistics in PG14, and the parallel-worker counters in PG18. The source
//! maps those catalog layouts to four layout versions.
//!
//! Every layout also carries the same `pg_database`-derived columns, joined by
//! `oid = datid`: the `frozen_xid_age`/`min_mxid_age` wraparound ages,
//! `datconnlimit`, and the `datallowconn`/`datistemplate` flags. They are
//! `None` for the shared-objects row, which has no `pg_database` entry.

use crate::{Section, StrId, Ts};

/// Type `1_005_004`: `pg_stat_database` on PG 18 (V3 plus parallel-worker
/// counters).
///
/// One row per database, plus the `datid = 0` shared-objects row (PG12+) whose
/// `datname` is `None`. `ts` is one `statement_timestamp()` for the snapshot;
/// `numbackends` is the instantaneous connection count. The layout keeps it
/// nullable for the documented shared-row `NULL`, but PG12+ system view
/// definitions return `0` for that row. `blk_read_time` / `blk_write_time` are zero unless
/// `track_io_timing` is on.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_005_004,
    name = "pg_stat_database",
    semantics = snapshot_full,
    sort_key("datid", "ts"),
    identity("datid")
)]
pub struct PgStatDatabaseV4 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Database oid; `0` for the shared-objects row.
    #[column(l)]
    pub datid: u32,
    /// Database name; `None` for the shared-objects row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Backends currently connected to this database; nullable for documented shared-row behavior.
    #[column(g, unit = count)]
    pub numbackends: Option<i32>,
    /// Committed transactions.
    #[column(c, unit = count)]
    pub xact_commit: i64,
    /// Rolled-back transactions.
    #[column(c, unit = count)]
    pub xact_rollback: i64,
    /// Disk blocks read.
    #[column(c, unit = count)]
    pub blks_read: i64,
    /// Buffer hits (blocks found in cache).
    #[column(c, unit = count)]
    pub blks_hit: i64,
    /// Rows returned by queries.
    #[column(c, unit = count)]
    pub tup_returned: i64,
    /// Rows fetched by queries.
    #[column(c, unit = count)]
    pub tup_fetched: i64,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub tup_inserted: i64,
    /// Rows updated.
    #[column(c, unit = count)]
    pub tup_updated: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub tup_deleted: i64,
    /// Queries cancelled due to recovery conflicts.
    #[column(c, unit = count)]
    pub conflicts: i64,
    /// Temporary files created by queries.
    #[column(c, unit = count)]
    pub temp_files: i64,
    /// Bytes written to temporary files.
    #[column(c, unit = bytes)]
    pub temp_bytes: i64,
    /// Deadlocks detected.
    #[column(c, unit = count)]
    pub deadlocks: i64,
    /// Time spent reading blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time spent writing blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// Time of the last statistics reset for this database; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
    /// Age of `datfrozenxid` in transactions; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub frozen_xid_age: Option<i64>,
    /// Age of `datminmxid` in multixacts; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub min_mxid_age: Option<i64>,
    /// Per-database connection limit; `-1` is unlimited, `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub datconnlimit: Option<i32>,
    /// Whether the database accepts connections; `None` for the shared-objects row.
    #[column(l)]
    pub datallowconn: Option<bool>,
    /// Whether the database is a template; `None` for the shared-objects row.
    #[column(l)]
    pub datistemplate: Option<bool>,
    /// Data-page checksum failures; `None` when data checksums are disabled.
    #[column(c, unit = count)]
    pub checksum_failures: Option<i64>,
    /// Time of the last checksum failure; `None` if there has been none.
    #[column(g, unit = microseconds)]
    pub checksum_last_failure: Option<Ts>,
    /// Time spent by sessions, ms.
    #[column(c, unit = milliseconds)]
    pub session_time: f64,
    /// Time sessions spent executing, ms.
    #[column(c, unit = milliseconds)]
    pub active_time: f64,
    /// Time sessions spent idle in transaction, ms.
    #[column(c, unit = milliseconds)]
    pub idle_in_transaction_time: f64,
    /// Sessions established.
    #[column(c, unit = count)]
    pub sessions: i64,
    /// Sessions lost to a dropped client connection.
    #[column(c, unit = count)]
    pub sessions_abandoned: i64,
    /// Sessions terminated by a fatal error.
    #[column(c, unit = count)]
    pub sessions_fatal: i64,
    /// Sessions terminated by operator action.
    #[column(c, unit = count)]
    pub sessions_killed: i64,
    /// Parallel workers planned for launch.
    #[column(c, unit = count)]
    pub parallel_workers_to_launch: i64,
    /// Parallel workers actually launched.
    #[column(c, unit = count)]
    pub parallel_workers_launched: i64,
}

/// Type `1_005_003`: `pg_stat_database` on PG 14-17 (V2 plus session
/// statistics, no parallel-worker counters). Column meanings match
/// [`PgStatDatabaseV4`] for fields present in this layout.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_005_003,
    name = "pg_stat_database",
    semantics = snapshot_full,
    sort_key("datid", "ts"),
    identity("datid")
)]
pub struct PgStatDatabaseV3 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Database oid; `0` for the shared-objects row.
    #[column(l)]
    pub datid: u32,
    /// Database name; `None` for the shared-objects row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Backends currently connected to this database; nullable for documented shared-row behavior.
    #[column(g, unit = count)]
    pub numbackends: Option<i32>,
    /// Committed transactions.
    #[column(c, unit = count)]
    pub xact_commit: i64,
    /// Rolled-back transactions.
    #[column(c, unit = count)]
    pub xact_rollback: i64,
    /// Disk blocks read.
    #[column(c, unit = count)]
    pub blks_read: i64,
    /// Buffer hits (blocks found in cache).
    #[column(c, unit = count)]
    pub blks_hit: i64,
    /// Rows returned by queries.
    #[column(c, unit = count)]
    pub tup_returned: i64,
    /// Rows fetched by queries.
    #[column(c, unit = count)]
    pub tup_fetched: i64,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub tup_inserted: i64,
    /// Rows updated.
    #[column(c, unit = count)]
    pub tup_updated: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub tup_deleted: i64,
    /// Queries cancelled due to recovery conflicts.
    #[column(c, unit = count)]
    pub conflicts: i64,
    /// Temporary files created by queries.
    #[column(c, unit = count)]
    pub temp_files: i64,
    /// Bytes written to temporary files.
    #[column(c, unit = bytes)]
    pub temp_bytes: i64,
    /// Deadlocks detected.
    #[column(c, unit = count)]
    pub deadlocks: i64,
    /// Time spent reading blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time spent writing blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// Time of the last statistics reset for this database; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
    /// Age of `datfrozenxid` in transactions; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub frozen_xid_age: Option<i64>,
    /// Age of `datminmxid` in multixacts; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub min_mxid_age: Option<i64>,
    /// Per-database connection limit; `-1` is unlimited, `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub datconnlimit: Option<i32>,
    /// Whether the database accepts connections; `None` for the shared-objects row.
    #[column(l)]
    pub datallowconn: Option<bool>,
    /// Whether the database is a template; `None` for the shared-objects row.
    #[column(l)]
    pub datistemplate: Option<bool>,
    /// Data-page checksum failures; `None` when data checksums are disabled.
    #[column(c, unit = count)]
    pub checksum_failures: Option<i64>,
    /// Time of the last checksum failure; `None` if there has been none.
    #[column(g, unit = microseconds)]
    pub checksum_last_failure: Option<Ts>,
    /// Time spent by sessions, ms.
    #[column(c, unit = milliseconds)]
    pub session_time: f64,
    /// Time sessions spent executing, ms.
    #[column(c, unit = milliseconds)]
    pub active_time: f64,
    /// Time sessions spent idle in transaction, ms.
    #[column(c, unit = milliseconds)]
    pub idle_in_transaction_time: f64,
    /// Sessions established.
    #[column(c, unit = count)]
    pub sessions: i64,
    /// Sessions lost to a dropped client connection.
    #[column(c, unit = count)]
    pub sessions_abandoned: i64,
    /// Sessions terminated by a fatal error.
    #[column(c, unit = count)]
    pub sessions_fatal: i64,
    /// Sessions terminated by operator action.
    #[column(c, unit = count)]
    pub sessions_killed: i64,
}

/// Type `1_005_002`: `pg_stat_database` on PG 12-13 (V1 plus checksum columns,
/// no session statistics). Column meanings match [`PgStatDatabaseV4`] for
/// fields present in this layout.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_005_002,
    name = "pg_stat_database",
    semantics = snapshot_full,
    sort_key("datid", "ts"),
    identity("datid")
)]
pub struct PgStatDatabaseV2 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Database oid; `0` for the shared-objects row.
    #[column(l)]
    pub datid: u32,
    /// Database name; `None` for the shared-objects row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Backends currently connected to this database; nullable for documented shared-row behavior.
    #[column(g, unit = count)]
    pub numbackends: Option<i32>,
    /// Committed transactions.
    #[column(c, unit = count)]
    pub xact_commit: i64,
    /// Rolled-back transactions.
    #[column(c, unit = count)]
    pub xact_rollback: i64,
    /// Disk blocks read.
    #[column(c, unit = count)]
    pub blks_read: i64,
    /// Buffer hits (blocks found in cache).
    #[column(c, unit = count)]
    pub blks_hit: i64,
    /// Rows returned by queries.
    #[column(c, unit = count)]
    pub tup_returned: i64,
    /// Rows fetched by queries.
    #[column(c, unit = count)]
    pub tup_fetched: i64,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub tup_inserted: i64,
    /// Rows updated.
    #[column(c, unit = count)]
    pub tup_updated: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub tup_deleted: i64,
    /// Queries cancelled due to recovery conflicts.
    #[column(c, unit = count)]
    pub conflicts: i64,
    /// Temporary files created by queries.
    #[column(c, unit = count)]
    pub temp_files: i64,
    /// Bytes written to temporary files.
    #[column(c, unit = bytes)]
    pub temp_bytes: i64,
    /// Deadlocks detected.
    #[column(c, unit = count)]
    pub deadlocks: i64,
    /// Time spent reading blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time spent writing blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// Time of the last statistics reset for this database; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
    /// Age of `datfrozenxid` in transactions; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub frozen_xid_age: Option<i64>,
    /// Age of `datminmxid` in multixacts; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub min_mxid_age: Option<i64>,
    /// Per-database connection limit; `-1` is unlimited, `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub datconnlimit: Option<i32>,
    /// Whether the database accepts connections; `None` for the shared-objects row.
    #[column(l)]
    pub datallowconn: Option<bool>,
    /// Whether the database is a template; `None` for the shared-objects row.
    #[column(l)]
    pub datistemplate: Option<bool>,
    /// Data-page checksum failures; `None` when data checksums are disabled.
    #[column(c, unit = count)]
    pub checksum_failures: Option<i64>,
    /// Time of the last checksum failure; `None` if there has been none.
    #[column(g, unit = microseconds)]
    pub checksum_last_failure: Option<Ts>,
}

/// Type `1_005_001`: `pg_stat_database` on PG 10-11 (base layout, no checksum or
/// session columns). Column meanings match [`PgStatDatabaseV4`] for fields
/// present in this layout.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 1_005_001,
    name = "pg_stat_database",
    semantics = snapshot_full,
    sort_key("datid", "ts"),
    identity("datid")
)]
pub struct PgStatDatabaseV1 {
    /// Snapshot time, unix microseconds; one value for all rows of a snapshot.
    #[column(t)]
    pub ts: Ts,
    /// Database oid; `0` for the shared-objects row.
    #[column(l)]
    pub datid: u32,
    /// Database name; `None` for the shared-objects row.
    #[column(l)]
    pub datname: Option<StrId>,
    /// Backends currently connected to this database; nullable for documented shared-row behavior.
    #[column(g, unit = count)]
    pub numbackends: Option<i32>,
    /// Committed transactions.
    #[column(c, unit = count)]
    pub xact_commit: i64,
    /// Rolled-back transactions.
    #[column(c, unit = count)]
    pub xact_rollback: i64,
    /// Disk blocks read.
    #[column(c, unit = count)]
    pub blks_read: i64,
    /// Buffer hits (blocks found in cache).
    #[column(c, unit = count)]
    pub blks_hit: i64,
    /// Rows returned by queries.
    #[column(c, unit = count)]
    pub tup_returned: i64,
    /// Rows fetched by queries.
    #[column(c, unit = count)]
    pub tup_fetched: i64,
    /// Rows inserted.
    #[column(c, unit = count)]
    pub tup_inserted: i64,
    /// Rows updated.
    #[column(c, unit = count)]
    pub tup_updated: i64,
    /// Rows deleted.
    #[column(c, unit = count)]
    pub tup_deleted: i64,
    /// Queries cancelled due to recovery conflicts.
    #[column(c, unit = count)]
    pub conflicts: i64,
    /// Temporary files created by queries.
    #[column(c, unit = count)]
    pub temp_files: i64,
    /// Bytes written to temporary files.
    #[column(c, unit = bytes)]
    pub temp_bytes: i64,
    /// Deadlocks detected.
    #[column(c, unit = count)]
    pub deadlocks: i64,
    /// Time spent reading blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_read_time: f64,
    /// Time spent writing blocks, ms; zero without `track_io_timing`.
    #[column(c, unit = milliseconds)]
    pub blk_write_time: f64,
    /// Time of the last statistics reset for this database; `None` if never.
    #[column(g, unit = microseconds)]
    pub stats_reset: Option<Ts>,
    /// Age of `datfrozenxid` in transactions; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub frozen_xid_age: Option<i64>,
    /// Age of `datminmxid` in multixacts; `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub min_mxid_age: Option<i64>,
    /// Per-database connection limit; `-1` is unlimited, `None` for the shared-objects row.
    #[column(g, unit = count)]
    pub datconnlimit: Option<i32>,
    /// Whether the database accepts connections; `None` for the shared-objects row.
    #[column(l)]
    pub datallowconn: Option<bool>,
    /// Whether the database is a template; `None` for the shared-objects row.
    #[column(l)]
    pub datistemplate: Option<bool>,
}

#[cfg(test)]
#[path = "../tests/codec/pg_stat_database.rs"]
mod tests;
