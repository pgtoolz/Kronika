//! Types `2_001_001`–`2_007_001`: what a `PostgreSQL` log carries.
//!
//! Seven sections, one per shape the log has. Errors and slow statements
//! arrive grouped, because a log that repeats one message ten thousand times
//! says the same thing as a count of ten thousand and costs ten thousand rows
//! to say it. The other five are one row per record.
//!
//! A nullable numeric column means the release or the message shape did not
//! print that number, not that `PostgreSQL` reported zero.

use crate::{Section, StrId, Ts};

/// Type `2_001_001`: `PostgreSQL` errors, grouped by what they say.
///
/// One row is one `(severity, category, pattern)` group in the collection
/// window, where the pattern is the message with its values replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 2_001_001,
    name = "pg_log_errors",
    semantics = event_stream,
    sort_key("severity", "category", "pattern", "ts")
)]
pub struct PgLogErrors {
    /// Time of the first occurrence, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// `0` error, `1` fatal, `2` panic, `3` warning, `4` log.
    #[column(l)]
    pub severity: u8,
    /// `0` lock through `10` other; the taxonomy is in
    /// `docs/type-registry/postgresql.md`.
    #[column(l)]
    pub category: u8,
    /// `SQLSTATE`, when the format carried one.
    #[column(l)]
    pub sqlstate: Option<StrId>,
    /// The normalized message the group is keyed on.
    #[column(l)]
    pub pattern: StrId,
    /// Occurrences in the collection window.
    #[column(g, unit = count)]
    pub count: u32,
    /// The first occurrence's message, with its values intact.
    #[column(l)]
    pub sample: StrId,
    /// `DETAIL` of the first occurrence.
    #[column(l)]
    pub detail: Option<StrId>,
    /// `HINT` of the first occurrence.
    #[column(l)]
    pub hint: Option<StrId>,
    /// `CONTEXT` of the first occurrence.
    #[column(l)]
    pub context: Option<StrId>,
    /// The statement the first occurrence was raised under.
    #[column(l)]
    pub statement: Option<StrId>,
    /// Database; `NULL` when `stderr` carries no `%d`.
    #[column(l)]
    pub database: Option<StrId>,
    /// User; `NULL` when `stderr` carries no `%u`.
    #[column(l)]
    pub username: Option<StrId>,
}

/// Type `2_002_001`: checkpoint records.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 2_002_001,
    name = "pg_log_checkpoints",
    semantics = event_stream,
    sort_key("ts", "phase")
)]
pub struct PgLogCheckpoints {
    /// Record time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// `0` starting, `1` complete, `2` too frequent.
    #[column(l)]
    pub phase: u8,
    /// The starting reason, or the too-frequent warning text.
    #[column(l)]
    pub reason: Option<StrId>,
    /// Interval the too-frequent warning names.
    #[column(g, unit = seconds)]
    pub seconds_apart: Option<i64>,
    /// Buffers written by the checkpoint.
    #[column(g, unit = count)]
    pub buffers_written: Option<i64>,
    /// Write phase.
    #[column(g, unit = milliseconds)]
    pub write_ms: Option<f64>,
    /// Sync phase.
    #[column(g, unit = milliseconds)]
    pub sync_ms: Option<f64>,
    /// Whole checkpoint.
    #[column(g, unit = milliseconds)]
    pub total_ms: Option<f64>,
    /// WAL distance.
    #[column(g, unit = kib)]
    pub distance_kb: Option<i64>,
    /// Estimated WAL distance.
    #[column(g, unit = kib)]
    pub estimate_kb: Option<i64>,
    /// WAL files added.
    #[column(g, unit = count)]
    pub wal_added: Option<i64>,
    /// WAL files removed.
    #[column(g, unit = count)]
    pub wal_removed: Option<i64>,
    /// WAL files recycled.
    #[column(g, unit = count)]
    pub wal_recycled: Option<i64>,
    /// Files synced.
    #[column(g, unit = count)]
    pub sync_files: Option<i64>,
    /// Longest single file sync.
    #[column(g, unit = milliseconds)]
    pub longest_sync_ms: Option<f64>,
    /// Average file sync.
    #[column(g, unit = milliseconds)]
    pub average_sync_ms: Option<f64>,
}

/// Type `2_003_001`: autovacuum and autoanalyze reports.
///
/// An analyze report prints no tuple counts, so those columns are `NULL` on
/// every `kind = 1` row.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 2_003_001,
    name = "pg_log_autovacuum",
    semantics = event_stream,
    sort_key("ts", "kind", "relation")
)]
pub struct PgLogAutovacuum {
    /// Record time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// `0` vacuum, `1` analyze.
    #[column(l)]
    pub kind: u8,
    /// The table, as the server named it.
    #[column(l)]
    pub relation: Option<StrId>,
    /// Index scans.
    #[column(g, unit = count)]
    pub index_scans: Option<i64>,
    /// Heap pages removed.
    #[column(g, unit = pages)]
    pub pages_removed: Option<i64>,
    /// Heap pages left.
    #[column(g, unit = pages)]
    pub pages_remaining: Option<i64>,
    /// Tuples removed.
    #[column(g, unit = count)]
    pub tuples_removed: Option<i64>,
    /// Tuples left.
    #[column(g, unit = count)]
    pub tuples_remaining: Option<i64>,
    /// Dead tuples an open transaction still holds down.
    #[column(g, unit = count)]
    pub tuples_dead_not_removable: Option<i64>,
    /// Runtime.
    #[column(g, unit = milliseconds)]
    pub elapsed_ms: Option<f64>,
    /// Buffer hits.
    #[column(g, unit = count)]
    pub buffer_hits: Option<i64>,
    /// Buffer misses.
    #[column(g, unit = count)]
    pub buffer_misses: Option<i64>,
    /// Buffers dirtied.
    #[column(g, unit = count)]
    pub buffer_dirtied: Option<i64>,
    /// Average read rate.
    #[column(g, unit = megabytes_per_second)]
    pub avg_read_rate_mbs: Option<f64>,
    /// Average write rate.
    #[column(g, unit = megabytes_per_second)]
    pub avg_write_rate_mbs: Option<f64>,
    /// User CPU.
    #[column(g, unit = milliseconds)]
    pub cpu_user_ms: Option<f64>,
    /// System CPU.
    #[column(g, unit = milliseconds)]
    pub cpu_system_ms: Option<f64>,
    /// WAL records generated.
    #[column(g, unit = count)]
    pub wal_records: Option<i64>,
    /// WAL full-page images generated.
    #[column(g, unit = count)]
    pub wal_fpi: Option<i64>,
    /// WAL generated.
    #[column(g, unit = bytes)]
    pub wal_bytes: Option<i64>,
}

/// Type `2_004_001`: statements `log_min_duration_statement` reported,
/// grouped by the statement with its literals replaced.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 2_004_001,
    name = "pg_log_slow_queries",
    semantics = event_stream,
    sort_key("pattern", "ts")
)]
pub struct PgLogSlowQueries {
    /// Time of the slowest occurrence, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// The normalized statement the group is keyed on.
    #[column(l)]
    pub pattern: StrId,
    /// The slowest occurrence, with its literals intact.
    #[column(l)]
    pub sample: StrId,
    /// Occurrences in the collection window.
    #[column(g, unit = count)]
    pub count: u32,
    /// The slowest occurrence.
    #[column(g, unit = milliseconds)]
    pub max_duration_ms: f64,
    /// Sum of the group's durations.
    #[column(g, unit = milliseconds)]
    pub total_duration_ms: f64,
}

/// Type `2_005_002`: `log_lock_waits` records.
///
/// `PostgreSQL` writes the wait and the acquisition as separate records with
/// different continuations, so they are separate rows here too.
#[derive(Debug, Clone, Copy, PartialEq, Section)]
#[section(
    id = 2_005_002,
    name = "pg_log_lock_waits",
    semantics = event_stream,
    sort_key("ts", "kind", "pid")
)]
pub struct PgLogLockWaits {
    /// Record time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// `0` still waiting, `1` acquired.
    #[column(l)]
    pub kind: u8,
    /// The waiting backend.
    #[column(l)]
    pub pid: Option<i32>,
    /// Lock mode, such as `ShareLock`.
    #[column(l)]
    pub lock_mode: Option<StrId>,
    /// What was locked, such as `transaction 12345`.
    #[column(l)]
    pub lock_target: Option<StrId>,
    /// How long the wait lasted.
    #[column(g, unit = milliseconds)]
    pub duration_ms: Option<f64>,
    /// The holder PID list from `DETAIL`.
    #[column(l)]
    pub holding_pids: Option<StrId>,
    /// The wait-queue PID list from `DETAIL`.
    #[column(l)]
    pub wait_queue: Option<StrId>,
    /// `DETAIL`, which names the holder and the queue.
    #[column(l)]
    pub detail: Option<StrId>,
    /// `CONTEXT`.
    #[column(l)]
    pub context: Option<StrId>,
    /// The statement that waited.
    #[column(l)]
    pub statement: Option<StrId>,
}

/// Type `2_006_001`: server lifecycle records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 2_006_001,
    name = "pg_log_lifecycle",
    semantics = event_stream,
    sort_key("ts", "kind")
)]
pub struct PgLogLifecycle {
    /// Record time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// `0` crash, `1` shutdown, `2` ready.
    #[column(l)]
    pub kind: u8,
    /// The process that crashed.
    #[column(l)]
    pub pid: Option<i32>,
    /// The signal that killed it.
    #[column(l)]
    pub signal: Option<i32>,
    /// `fast`, `smart` or `immediate`.
    #[column(l)]
    pub shutdown_mode: Option<StrId>,
    /// The record itself.
    #[column(l)]
    pub message: StrId,
    /// The statement a crash `DETAIL` says the process was running.
    #[column(l)]
    pub query_detail: Option<StrId>,
}

/// Type `2_007_001`: `log_temp_files` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 2_007_001,
    name = "pg_log_temp_files",
    semantics = event_stream,
    sort_key("ts", "size_bytes")
)]
pub struct PgLogTempFiles {
    /// Record time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// The cluster that wrote the line, from `pg_control_system()`. Null when
    /// the log was named outright and no server was asked.
    #[column(l)]
    pub system_identifier: Option<u64>,
    /// The file the line was read from.
    #[column(l)]
    pub source_file: StrId,
    /// The file the server wrote.
    #[column(l)]
    pub path: Option<StrId>,
    /// Its size.
    #[column(g, unit = bytes)]
    pub size_bytes: i64,
    /// The statement that needed it.
    #[column(l)]
    pub statement: Option<StrId>,
}

#[cfg(test)]
mod tests;
