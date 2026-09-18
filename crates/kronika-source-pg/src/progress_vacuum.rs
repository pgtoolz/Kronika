//! `pg_stat_progress_vacuum` collection for types `1_012_004` through
//! `1_012_006`.
//!
//! One row per backend running `VACUUM`. An empty view produces no section.
//! Query shape follows the server major; the caller interns `datname` and
//! `phase`. `relid` is a `pg_class` OID, effectively never reused, so it
//! stays the row's identity across an hour whether or not the name resolves.
//! `schemaname`/`relname` are joined from `pg_class`/`pg_namespace` in the
//! same query: the join finds a row only when the vacuumed relation belongs
//! to the database this connection is on, since a session sees only its own
//! database's catalog. `pg_class` OIDs are assigned from one cluster-wide
//! counter, so a match is never a different relation's name; an unresolved
//! name is left `NULL`, not guessed.

use kronika_registry::pg_stat_progress_vacuum::{
    PgStatProgressVacuumV1, PgStatProgressVacuumV2, PgStatProgressVacuumV3,
};
use kronika_registry::{StrId, Ts};
use tokio_postgres::types::Type;

use crate::query::{self, Batch, BatchError, BatchWrite, QueryStats};
use crate::{Session, intern_opt as opt};

/// The exact `pg_stat_progress_vacuum` column set for one server major.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressVacuumVersion {
    /// PG10-16: tuple-count dead-tuple columns, type `1_012_004`.
    V1,
    /// PG17: byte-based TID store and index-progress counters, type `1_012_005`.
    V2,
    /// PG18+: PG17 columns plus `delay_time`, type `1_012_006`.
    V3,
}

/// Map a server major to its exact column set.
#[must_use]
pub const fn progress_vacuum_version(major: u32) -> ProgressVacuumVersion {
    if major >= 18 {
        ProgressVacuumVersion::V3
    } else if major >= 17 {
        ProgressVacuumVersion::V2
    } else {
        ProgressVacuumVersion::V1
    }
}

/// SQL for one exact column set.
///
/// `ts` is one `statement_timestamp()` for the whole snapshot. `PostgreSQL`'s
/// PG10-16 view has the V1 projection; PG17 replaces its two dead-tuple-count
/// columns with five byte/item/index columns; PG18 adds `delay_time`.
#[must_use]
pub const fn progress_vacuum_query(version: ProgressVacuumVersion) -> &'static str {
    match version {
        ProgressVacuumVersion::V1 => marked!(
            "SELECT v.pid, v.datid, v.datname, v.relid, \
             n.nspname::text AS schemaname, c.relname::text AS relname, \
             COALESCE(a.backend_type = 'autovacuum worker', false) AS is_autovacuum, v.phase, \
             v.heap_blks_total, v.heap_blks_scanned, v.heap_blks_vacuumed, \
             v.index_vacuum_count, v.max_dead_tuples, v.num_dead_tuples, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_progress_vacuum v \
             LEFT JOIN pg_stat_activity a ON a.pid = v.pid \
             LEFT JOIN pg_class c ON c.oid = v.relid \
             LEFT JOIN pg_namespace n ON n.oid = c.relnamespace"
        ),
        ProgressVacuumVersion::V2 => marked!(
            "SELECT v.pid, v.datid, v.datname, v.relid, \
             n.nspname::text AS schemaname, c.relname::text AS relname, \
             COALESCE(a.backend_type = 'autovacuum worker', false) AS is_autovacuum, v.phase, \
             v.heap_blks_total, v.heap_blks_scanned, v.heap_blks_vacuumed, \
             v.index_vacuum_count, v.max_dead_tuple_bytes, v.dead_tuple_bytes, \
             v.num_dead_item_ids, v.indexes_total, v.indexes_processed, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_progress_vacuum v \
             LEFT JOIN pg_stat_activity a ON a.pid = v.pid \
             LEFT JOIN pg_class c ON c.oid = v.relid \
             LEFT JOIN pg_namespace n ON n.oid = c.relnamespace"
        ),
        ProgressVacuumVersion::V3 => marked!(
            "SELECT v.pid, v.datid, v.datname, v.relid, \
             n.nspname::text AS schemaname, c.relname::text AS relname, \
             COALESCE(a.backend_type = 'autovacuum worker', false) AS is_autovacuum, v.phase, \
             v.heap_blks_total, v.heap_blks_scanned, v.heap_blks_vacuumed, \
             v.index_vacuum_count, v.max_dead_tuple_bytes, v.dead_tuple_bytes, \
             v.num_dead_item_ids, v.indexes_total, v.indexes_processed, v.delay_time, \
             (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
             FROM pg_stat_progress_vacuum v \
             LEFT JOIN pg_stat_activity a ON a.pid = v.pid \
             LEFT JOIN pg_class c ON c.oid = v.relid \
             LEFT JOIN pg_namespace n ON n.oid = c.relnamespace"
        ),
    }
}

/// One exact source row, tagged with the server layout that produced it.
#[derive(Debug)]
pub enum ProgressVacuumRow {
    /// PG10-16 row.
    V1(ProgressVacuumV1Row),
    /// PG17 row.
    V2(ProgressVacuumV2Row),
    /// PG18+ row.
    V3(ProgressVacuumV3Row),
}

/// Raw PG10-16 row before interning.
#[derive(Debug)]
pub struct ProgressVacuumV1Row {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Backend process id.
    pub pid: i32,
    /// Database OID.
    pub datid: u32,
    /// Database name.
    pub datname: String,
    /// Table OID.
    pub relid: u32,
    /// Schema of the table, when `pg_class` resolves it from this connection.
    pub schemaname: Option<String>,
    /// Table name, when `pg_class` resolves it from this connection.
    pub relname: Option<String>,
    /// Whether the backend is an autovacuum worker.
    pub is_autovacuum: bool,
    /// Vacuum phase.
    pub phase: String,
    /// Heap blocks in the table at scan start.
    pub heap_blks_total: i64,
    /// Heap blocks scanned in this vacuum.
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed in this vacuum.
    pub heap_blks_vacuumed: i64,
    /// Index-vacuum cycles completed.
    pub index_vacuum_count: i64,
    /// Dead-tuple capacity, count.
    pub max_dead_tuples: i64,
    /// Dead tuples collected, count.
    pub num_dead_tuples: i64,
}

/// Raw PG17 row before interning.
#[derive(Debug)]
pub struct ProgressVacuumV2Row {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Backend process id.
    pub pid: i32,
    /// Database OID.
    pub datid: u32,
    /// Database name.
    pub datname: String,
    /// Table OID.
    pub relid: u32,
    /// Schema of the table, when `pg_class` resolves it from this connection.
    pub schemaname: Option<String>,
    /// Table name, when `pg_class` resolves it from this connection.
    pub relname: Option<String>,
    /// Whether the backend is an autovacuum worker.
    pub is_autovacuum: bool,
    /// Vacuum phase.
    pub phase: String,
    /// Heap blocks in the table at scan start.
    pub heap_blks_total: i64,
    /// Heap blocks scanned in this vacuum.
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed in this vacuum.
    pub heap_blks_vacuumed: i64,
    /// Index-vacuum cycles completed.
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    pub num_dead_item_ids: i64,
    /// Indexes to process.
    pub indexes_total: i64,
    /// Indexes processed.
    pub indexes_processed: i64,
}

/// Raw PG18+ row before interning.
#[derive(Debug)]
pub struct ProgressVacuumV3Row {
    /// Snapshot time, unix microseconds.
    pub ts: i64,
    /// Backend process id.
    pub pid: i32,
    /// Database OID.
    pub datid: u32,
    /// Database name.
    pub datname: String,
    /// Table OID.
    pub relid: u32,
    /// Schema of the table, when `pg_class` resolves it from this connection.
    pub schemaname: Option<String>,
    /// Table name, when `pg_class` resolves it from this connection.
    pub relname: Option<String>,
    /// Whether the backend is an autovacuum worker.
    pub is_autovacuum: bool,
    /// Vacuum phase.
    pub phase: String,
    /// Heap blocks in the table at scan start.
    pub heap_blks_total: i64,
    /// Heap blocks scanned in this vacuum.
    pub heap_blks_scanned: i64,
    /// Heap blocks vacuumed in this vacuum.
    pub heap_blks_vacuumed: i64,
    /// Index-vacuum cycles completed.
    pub index_vacuum_count: i64,
    /// Dead-tuple TID store capacity, bytes.
    pub max_dead_tuple_bytes: i64,
    /// Dead-tuple TID store usage, bytes.
    pub dead_tuple_bytes: i64,
    /// Dead item identifiers collected.
    pub num_dead_item_ids: i64,
    /// Indexes to process.
    pub indexes_total: i64,
    /// Indexes processed.
    pub indexes_processed: i64,
    /// Cost-delay sleep time, milliseconds.
    pub delay_time: f64,
}

/// Build a type `1_012_004` row, interning `datname`, `phase` and the
/// resolved relation name.
///
/// # Errors
/// Returns the interner's error if a label cannot be interned.
pub fn to_v1<E>(
    row: &ProgressVacuumV1Row,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatProgressVacuumV1, E> {
    Ok(PgStatProgressVacuumV1 {
        ts: Ts(row.ts),
        pid: row.pid,
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: opt(&mut intern, row.schemaname.as_deref())?,
        relname: opt(&mut intern, row.relname.as_deref())?,
        is_autovacuum: row.is_autovacuum,
        phase: intern(row.phase.as_bytes())?,
        heap_blks_total: row.heap_blks_total,
        heap_blks_scanned: row.heap_blks_scanned,
        heap_blks_vacuumed: row.heap_blks_vacuumed,
        index_vacuum_count: row.index_vacuum_count,
        max_dead_tuples: row.max_dead_tuples,
        num_dead_tuples: row.num_dead_tuples,
    })
}

/// Build a type `1_012_005` row, interning `datname`, `phase` and the
/// resolved relation name.
///
/// # Errors
/// Returns the interner's error if a label cannot be interned.
pub fn to_v2<E>(
    row: &ProgressVacuumV2Row,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatProgressVacuumV2, E> {
    Ok(PgStatProgressVacuumV2 {
        ts: Ts(row.ts),
        pid: row.pid,
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: opt(&mut intern, row.schemaname.as_deref())?,
        relname: opt(&mut intern, row.relname.as_deref())?,
        is_autovacuum: row.is_autovacuum,
        phase: intern(row.phase.as_bytes())?,
        heap_blks_total: row.heap_blks_total,
        heap_blks_scanned: row.heap_blks_scanned,
        heap_blks_vacuumed: row.heap_blks_vacuumed,
        index_vacuum_count: row.index_vacuum_count,
        max_dead_tuple_bytes: row.max_dead_tuple_bytes,
        dead_tuple_bytes: row.dead_tuple_bytes,
        num_dead_item_ids: row.num_dead_item_ids,
        indexes_total: row.indexes_total,
        indexes_processed: row.indexes_processed,
    })
}

/// Build a type `1_012_006` row, interning `datname`, `phase` and the
/// resolved relation name.
///
/// # Errors
/// Returns the interner's error if a label cannot be interned.
pub fn to_v3<E>(
    row: &ProgressVacuumV3Row,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgStatProgressVacuumV3, E> {
    Ok(PgStatProgressVacuumV3 {
        ts: Ts(row.ts),
        pid: row.pid,
        datid: row.datid,
        datname: intern(row.datname.as_bytes())?,
        relid: row.relid,
        schemaname: opt(&mut intern, row.schemaname.as_deref())?,
        relname: opt(&mut intern, row.relname.as_deref())?,
        is_autovacuum: row.is_autovacuum,
        phase: intern(row.phase.as_bytes())?,
        heap_blks_total: row.heap_blks_total,
        heap_blks_scanned: row.heap_blks_scanned,
        heap_blks_vacuumed: row.heap_blks_vacuumed,
        index_vacuum_count: row.index_vacuum_count,
        max_dead_tuple_bytes: row.max_dead_tuple_bytes,
        dead_tuple_bytes: row.dead_tuple_bytes,
        num_dead_item_ids: row.num_dead_item_ids,
        indexes_total: row.indexes_total,
        indexes_processed: row.indexes_processed,
        delay_time: row.delay_time,
    })
}

fn row_from_pg(
    row: query::IndexedRow<'_>,
    version: ProgressVacuumVersion,
) -> anyhow::Result<ProgressVacuumRow> {
    Ok(match version {
        ProgressVacuumVersion::V1 => ProgressVacuumRow::V1(ProgressVacuumV1Row {
            ts: row.try_get("ts_us")?,
            pid: row.try_get("pid")?,
            datid: row.try_get("datid")?,
            datname: row.try_get("datname")?,
            relid: row.try_get("relid")?,
            schemaname: row.try_get("schemaname")?,
            relname: row.try_get("relname")?,
            is_autovacuum: row.try_get("is_autovacuum")?,
            phase: row.try_get("phase")?,
            heap_blks_total: row.try_get("heap_blks_total")?,
            heap_blks_scanned: row.try_get("heap_blks_scanned")?,
            heap_blks_vacuumed: row.try_get("heap_blks_vacuumed")?,
            index_vacuum_count: row.try_get("index_vacuum_count")?,
            max_dead_tuples: row.try_get("max_dead_tuples")?,
            num_dead_tuples: row.try_get("num_dead_tuples")?,
        }),
        ProgressVacuumVersion::V2 => ProgressVacuumRow::V2(ProgressVacuumV2Row {
            ts: row.try_get("ts_us")?,
            pid: row.try_get("pid")?,
            datid: row.try_get("datid")?,
            datname: row.try_get("datname")?,
            relid: row.try_get("relid")?,
            schemaname: row.try_get("schemaname")?,
            relname: row.try_get("relname")?,
            is_autovacuum: row.try_get("is_autovacuum")?,
            phase: row.try_get("phase")?,
            heap_blks_total: row.try_get("heap_blks_total")?,
            heap_blks_scanned: row.try_get("heap_blks_scanned")?,
            heap_blks_vacuumed: row.try_get("heap_blks_vacuumed")?,
            index_vacuum_count: row.try_get("index_vacuum_count")?,
            max_dead_tuple_bytes: row.try_get("max_dead_tuple_bytes")?,
            dead_tuple_bytes: row.try_get("dead_tuple_bytes")?,
            num_dead_item_ids: row.try_get("num_dead_item_ids")?,
            indexes_total: row.try_get("indexes_total")?,
            indexes_processed: row.try_get("indexes_processed")?,
        }),
        ProgressVacuumVersion::V3 => ProgressVacuumRow::V3(ProgressVacuumV3Row {
            ts: row.try_get("ts_us")?,
            pid: row.try_get("pid")?,
            datid: row.try_get("datid")?,
            datname: row.try_get("datname")?,
            relid: row.try_get("relid")?,
            schemaname: row.try_get("schemaname")?,
            relname: row.try_get("relname")?,
            is_autovacuum: row.try_get("is_autovacuum")?,
            phase: row.try_get("phase")?,
            heap_blks_total: row.try_get("heap_blks_total")?,
            heap_blks_scanned: row.try_get("heap_blks_scanned")?,
            heap_blks_vacuumed: row.try_get("heap_blks_vacuumed")?,
            index_vacuum_count: row.try_get("index_vacuum_count")?,
            max_dead_tuple_bytes: row.try_get("max_dead_tuple_bytes")?,
            dead_tuple_bytes: row.try_get("dead_tuple_bytes")?,
            num_dead_item_ids: row.try_get("num_dead_item_ids")?,
            indexes_total: row.try_get("indexes_total")?,
            indexes_processed: row.try_get("indexes_processed")?,
            delay_time: row.try_get("delay_time")?,
        }),
    })
}

/// Stream every in-progress vacuum in bounded batches.
///
/// # Errors
/// Returns the `PostgreSQL` stream error or the batch sink error.
pub async fn collect_progress_vacuum<E>(
    session: Session<'_>,
    major: u32,
    stats: &mut QueryStats,
    sink: impl FnMut(Batch<ProgressVacuumRow>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>> {
    let version = progress_vacuum_version(major);
    query::read_batched(
        session,
        progress_vacuum_query(version),
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
#[path = "tests/progress_vacuum.rs"]
mod tests;
