//! Turning `PostgreSQL` log records into the events the registry stores.
//!
//! `LOG` records are read for the six typed shapes below; everything at
//! `WARNING` and above is grouped into error patterns. A `LOG` record that
//! matches none of the shapes is dropped, because a log that reports every
//! connection would otherwise cost more than the events in it.

mod parse;

use parse::{
    crash_statement, detail_list, parse_autovacuum, parse_checkpoint, parse_lifecycle,
    parse_lock_wait, parse_slow_query, parse_temp_file,
};

use std::collections::HashMap;

use super::normalize::{ErrorCategory, classify_error, normalize_error, normalize_sql};
use super::{PgRecord, Severity};
use crate::text::{MAX_TEXT_BYTES, truncate};

/// Which part of a checkpoint a record reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointPhase {
    /// `checkpoint starting:`.
    Starting,
    /// `checkpoint complete:`.
    Complete,
    /// `checkpoints are occurring too frequently`.
    TooFrequent,
}

impl CheckpointPhase {
    /// The code stored in `pg_log_checkpoints.phase`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Starting => 0,
            Self::Complete => 1,
            Self::TooFrequent => 2,
        }
    }
}

/// Whether autovacuum vacuumed or analyzed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutovacuumKind {
    /// `automatic vacuum of table`.
    Vacuum,
    /// `automatic analyze of table`.
    Analyze,
}

impl AutovacuumKind {
    /// The code stored in `pg_log_autovacuum.kind`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Vacuum => 0,
            Self::Analyze => 1,
        }
    }
}

/// Whether a backend was still waiting for a lock or had just taken it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockWaitKind {
    /// `still waiting for`.
    Waiting,
    /// `acquired`.
    Acquired,
}

impl LockWaitKind {
    /// The code stored in `pg_log_lock_waits.kind`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Acquired => 1,
        }
    }
}

/// What happened to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleKind {
    /// A backend was killed or exited abnormally.
    Crash,
    /// A shutdown request arrived.
    Shutdown,
    /// The server is accepting connections.
    Ready,
}

impl LifecycleKind {
    /// The code stored in `pg_log_lifecycle.kind`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Crash => 0,
            Self::Shutdown => 1,
            Self::Ready => 2,
        }
    }
}

/// One `(severity, category, pattern)` group of errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorGroup {
    /// Time of the first occurrence, unix microseconds.
    pub ts: i64,
    /// Severity.
    pub severity: Severity,
    /// Category.
    pub category: ErrorCategory,
    /// `SQLSTATE` of the first occurrence.
    pub sqlstate: Option<String>,
    /// The normalized pattern the group is keyed on.
    pub pattern: String,
    /// Occurrences in this read.
    pub count: u32,
    /// The first occurrence's message, with its values intact.
    pub sample: String,
    /// `DETAIL` of the first occurrence.
    pub detail: Option<String>,
    /// `HINT` of the first occurrence.
    pub hint: Option<String>,
    /// `CONTEXT` of the first occurrence.
    pub context: Option<String>,
    /// The statement the first occurrence was raised under.
    pub statement: Option<String>,
    /// Database of the first occurrence.
    pub database: Option<String>,
    /// User of the first occurrence.
    pub username: Option<String>,
}

/// One checkpoint record.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointEvent {
    /// Record time, unix microseconds.
    pub ts: i64,
    /// Which part of the checkpoint this is.
    pub phase: CheckpointPhase,
    /// The starting reason, or the too-frequent warning text.
    pub reason: Option<String>,
    /// Seconds between the checkpoints the too-frequent warning names.
    pub seconds_apart: Option<i64>,
    /// Buffers written.
    pub buffers_written: Option<i64>,
    /// Write phase, ms.
    pub write_ms: Option<f64>,
    /// Sync phase, ms.
    pub sync_ms: Option<f64>,
    /// Whole checkpoint, ms.
    pub total_ms: Option<f64>,
    /// WAL distance, kB.
    pub distance_kb: Option<i64>,
    /// Estimated WAL distance, kB.
    pub estimate_kb: Option<i64>,
    /// WAL files added.
    pub wal_added: Option<i64>,
    /// WAL files removed.
    pub wal_removed: Option<i64>,
    /// WAL files recycled.
    pub wal_recycled: Option<i64>,
    /// Files synced.
    pub sync_files: Option<i64>,
    /// Longest single file sync, ms.
    pub longest_sync_ms: Option<f64>,
    /// Average file sync, ms.
    pub average_sync_ms: Option<f64>,
}

/// One autovacuum or autoanalyze report.
#[derive(Debug, Clone, PartialEq)]
pub struct AutovacuumEvent {
    /// Record time, unix microseconds.
    pub ts: i64,
    /// Vacuum or analyze.
    pub kind: AutovacuumKind,
    /// The table, as the server named it.
    pub relation: Option<String>,
    /// Index scans.
    pub index_scans: Option<i64>,
    /// Heap pages removed.
    pub pages_removed: Option<i64>,
    /// Heap pages left.
    pub pages_remaining: Option<i64>,
    /// Tuples removed.
    pub tuples_removed: Option<i64>,
    /// Tuples left.
    pub tuples_remaining: Option<i64>,
    /// Dead tuples that could not be removed yet.
    pub tuples_dead_not_removable: Option<i64>,
    /// Runtime, ms.
    pub elapsed_ms: Option<f64>,
    /// Buffer hits.
    pub buffer_hits: Option<i64>,
    /// Buffer misses.
    pub buffer_misses: Option<i64>,
    /// Buffers dirtied.
    pub buffer_dirtied: Option<i64>,
    /// Average read rate, MB/s.
    pub avg_read_rate_mbs: Option<f64>,
    /// Average write rate, MB/s.
    pub avg_write_rate_mbs: Option<f64>,
    /// User CPU, ms.
    pub cpu_user_ms: Option<f64>,
    /// System CPU, ms.
    pub cpu_system_ms: Option<f64>,
    /// WAL records generated.
    pub wal_records: Option<i64>,
    /// WAL full-page images generated.
    pub wal_fpi: Option<i64>,
    /// WAL bytes generated.
    pub wal_bytes: Option<i64>,
}

/// One normalized statement that `log_min_duration_statement` reported.
#[derive(Debug, Clone, PartialEq)]
pub struct SlowQuery {
    /// Time of the slowest occurrence, unix microseconds.
    pub ts: i64,
    /// The normalized statement the group is keyed on.
    pub pattern: String,
    /// The slowest occurrence's statement, with its values intact.
    pub sample: String,
    /// Occurrences in this read.
    pub count: u32,
    /// Slowest occurrence, ms.
    pub max_duration_ms: f64,
    /// Sum of the durations, ms.
    pub total_duration_ms: f64,
}

/// One `log_lock_waits` record.
#[derive(Debug, Clone, PartialEq)]
pub struct LockWait {
    /// Record time, unix microseconds.
    pub ts: i64,
    /// Waiting or acquired.
    pub kind: LockWaitKind,
    /// The waiting backend.
    pub pid: Option<i32>,
    /// Lock mode, such as `ShareLock`.
    pub lock_mode: Option<String>,
    /// What was locked, such as `transaction 12345`.
    pub lock_target: Option<String>,
    /// How long the wait lasted, ms.
    pub duration_ms: Option<f64>,
    /// `DETAIL`, which names the holder.
    pub detail: Option<String>,
    /// `CONTEXT`.
    pub context: Option<String>,
    /// The statement that waited.
    pub statement: Option<String>,
}

impl LockWait {
    /// The holder PID list from `DETAIL`.
    #[must_use]
    pub fn holding_pids(&self) -> Option<&str> {
        self.detail
            .as_deref()
            .and_then(|detail| detail_list(detail, "holding the lock: "))
    }

    /// The wait-queue PID list from `DETAIL`.
    #[must_use]
    pub fn wait_queue(&self) -> Option<&str> {
        self.detail
            .as_deref()
            .and_then(|detail| detail_list(detail, "Wait queue: "))
    }
}

/// One server lifecycle record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvent {
    /// Record time, unix microseconds.
    pub ts: i64,
    /// What happened.
    pub kind: LifecycleKind,
    /// The process that crashed.
    pub pid: Option<i32>,
    /// The signal that killed it.
    pub signal: Option<i32>,
    /// `fast`, `smart` or `immediate`.
    pub shutdown_mode: Option<String>,
    /// The record itself.
    pub message: String,
    /// The statement a crash `DETAIL` says the process was running.
    pub query_detail: Option<String>,
}

/// One `log_temp_files` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempFile {
    /// Record time, unix microseconds.
    pub ts: i64,
    /// The file the server wrote.
    pub path: Option<String>,
    /// Its size, bytes.
    pub size_bytes: i64,
    /// The statement that needed it.
    pub statement: Option<String>,
}

/// Everything one read of a `PostgreSQL` log produced.
#[derive(Debug, Default)]
pub struct Events {
    /// Error patterns, one row per group.
    pub errors: Vec<ErrorGroup>,
    /// Checkpoint records.
    pub checkpoints: Vec<CheckpointEvent>,
    /// Autovacuum and autoanalyze reports.
    pub autovacuum: Vec<AutovacuumEvent>,
    /// Slow statements, one row per normalized pattern.
    pub slow_queries: Vec<SlowQuery>,
    /// Lock waits.
    pub lock_waits: Vec<LockWait>,
    /// Lifecycle records.
    pub lifecycle: Vec<LifecycleEvent>,
    /// Temporary files.
    pub temp_files: Vec<TempFile>,
    grouped_errors: HashMap<(Severity, ErrorCategory, String), usize>,
    grouped_queries: HashMap<String, usize>,
}

impl Events {
    /// Whether this read produced nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.errors.is_empty()
            && self.checkpoints.is_empty()
            && self.autovacuum.is_empty()
            && self.slow_queries.is_empty()
            && self.lock_waits.is_empty()
            && self.lifecycle.is_empty()
            && self.temp_files.is_empty()
    }

    /// How many rows this read produced across every section.
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.errors.len()
            + self.checkpoints.len()
            + self.autovacuum.len()
            + self.slow_queries.len()
            + self.lock_waits.len()
            + self.lifecycle.len()
            + self.temp_files.len()
    }

    pub(super) fn add(&mut self, record: &PgRecord) {
        if record.severity == Severity::Log {
            self.add_log(record);
        } else {
            self.add_error(record);
        }
    }

    /// Release the grouping tables once a read is over.
    pub(super) fn finish(&mut self) {
        self.grouped_errors = HashMap::new();
        self.grouped_queries = HashMap::new();
    }

    fn add_log(&mut self, record: &PgRecord) {
        let message = record.message.as_str();
        if let Some(event) = parse_checkpoint(message, record.ts) {
            self.checkpoints.push(event);
        } else if let Some(event) = parse_autovacuum(message, record.ts) {
            self.autovacuum.push(event);
        } else if let Some((duration_ms, sql)) = parse_slow_query(message) {
            self.add_slow_query(record.ts, duration_ms, &sql);
        } else if let Some(mut event) = parse_lock_wait(message, record.ts) {
            event.detail.clone_from(&record.detail);
            event.context.clone_from(&record.context);
            event.statement.clone_from(&record.statement);
            self.lock_waits.push(event);
        } else if let Some(mut event) = parse_temp_file(message, record.ts) {
            event.statement.clone_from(&record.statement);
            self.temp_files.push(event);
        } else if let Some(mut event) = parse_lifecycle(message, record.ts) {
            event.query_detail = record.detail.as_deref().and_then(crash_statement);
            self.lifecycle.push(event);
        }
    }

    fn add_error(&mut self, record: &PgRecord) {
        let pattern = normalize_error(&record.message);
        let category = classify_error(&pattern, record.severity);
        let key = (record.severity, category, pattern.clone());
        if let Some(index) = self.grouped_errors.get(&key)
            && let Some(group) = self.errors.get_mut(*index)
        {
            group.count = group.count.saturating_add(1);
            return;
        }
        self.grouped_errors.insert(key, self.errors.len());
        self.errors.push(ErrorGroup {
            ts: record.ts,
            severity: record.severity,
            category,
            sqlstate: record.sqlstate.clone(),
            pattern,
            count: 1,
            sample: truncate(&record.message, MAX_TEXT_BYTES).to_owned(),
            detail: record.detail.clone(),
            hint: record.hint.clone(),
            context: record.context.clone(),
            statement: record.statement.clone(),
            database: record.database.clone(),
            username: record.username.clone(),
        });
    }

    fn add_slow_query(&mut self, ts: i64, duration_ms: f64, sql: &str) {
        let pattern = normalize_sql(sql);
        if let Some(index) = self.grouped_queries.get(&pattern)
            && let Some(group) = self.slow_queries.get_mut(*index)
        {
            group.count = group.count.saturating_add(1);
            group.total_duration_ms += duration_ms;
            if duration_ms > group.max_duration_ms {
                group.max_duration_ms = duration_ms;
                truncate(sql, MAX_TEXT_BYTES).clone_into(&mut group.sample);
                group.ts = ts;
            }
            return;
        }
        self.grouped_queries
            .insert(pattern.clone(), self.slow_queries.len());
        self.slow_queries.push(SlowQuery {
            ts,
            pattern,
            sample: truncate(sql, MAX_TEXT_BYTES).to_owned(),
            count: 1,
            max_duration_ms: duration_ms,
            total_duration_ms: duration_ms,
        });
    }
}

#[cfg(test)]
#[path = "../tests/postgres/events.rs"]
mod tests;
