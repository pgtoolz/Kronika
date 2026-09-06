//! Turning `PostgreSQL` log records into the events the registry stores.
//!
//! `LOG` records are read for the six typed shapes below; everything at
//! `WARNING` and above is grouped into error patterns. A `LOG` record that
//! matches none of the shapes is dropped, because a log that reports every
//! connection would otherwise cost more than the events in it.

use std::collections::HashMap;

use super::normalize::{ErrorCategory, classify_error, normalize_error, normalize_sql};
use super::{PgRecord, Severity};
use crate::text::{MAX_TEXT_BYTES, bounded, truncate};

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

const CHECKPOINT_STARTING: &str = "checkpoint starting:";
const CHECKPOINT_COMPLETE: &str = "checkpoint complete:";
const CHECKPOINT_TOO_FREQUENT: &str = "checkpoints are occurring too frequently";
const DURATION_PREFIX: &str = "duration: ";
const STATEMENT_MARKER: &str = " ms  statement: ";
const EXECUTE_MARKER: &str = " ms  execute ";
const TEMP_FILE_PREFIX: &str = "temporary file:";
const CRASH_PREFIX: &str = "server process (PID ";
const READY_MESSAGE: &str = "database system is ready to accept connections";
const SHUTDOWN_REQUESTS: &[(&str, &str)] = &[
    ("received fast shutdown request", "fast"),
    ("received smart shutdown request", "smart"),
    ("received immediate shutdown request", "immediate"),
];
const AUTOVACUUM_PREFIXES: &[(&str, AutovacuumKind)] = &[
    ("automatic vacuum of table", AutovacuumKind::Vacuum),
    (
        "automatic aggressive vacuum of table",
        AutovacuumKind::Vacuum,
    ),
    (
        "automatic vacuum to prevent wraparound of table",
        AutovacuumKind::Vacuum,
    ),
    (
        "automatic aggressive vacuum to prevent wraparound of table",
        AutovacuumKind::Vacuum,
    ),
    ("automatic analyze of table", AutovacuumKind::Analyze),
    (
        "automatic aggressive analyze of table",
        AutovacuumKind::Analyze,
    ),
];

fn parse_checkpoint(message: &str, ts: i64) -> Option<CheckpointEvent> {
    let empty = CheckpointEvent {
        ts,
        phase: CheckpointPhase::Starting,
        reason: None,
        seconds_apart: None,
        buffers_written: None,
        write_ms: None,
        sync_ms: None,
        total_ms: None,
        distance_kb: None,
        estimate_kb: None,
        wal_added: None,
        wal_removed: None,
        wal_recycled: None,
        sync_files: None,
        longest_sync_ms: None,
        average_sync_ms: None,
    };
    if let Some(reason) = message.strip_prefix(CHECKPOINT_STARTING) {
        return Some(CheckpointEvent {
            reason: bounded(reason),
            ..empty
        });
    }
    if message.starts_with(CHECKPOINT_COMPLETE) {
        let (wal_added, wal_removed, wal_recycled) = wal_file_counts(message);
        return Some(CheckpointEvent {
            phase: CheckpointPhase::Complete,
            buffers_written: i64_after(message, "wrote "),
            write_ms: seconds_as_ms(message, "write="),
            sync_ms: seconds_as_ms(message, "sync="),
            total_ms: seconds_as_ms(message, "total="),
            distance_kb: i64_after(message, "distance="),
            estimate_kb: i64_after(message, "estimate="),
            wal_added,
            wal_removed,
            wal_recycled,
            sync_files: i64_after(message, "sync files="),
            longest_sync_ms: seconds_as_ms(message, "longest="),
            average_sync_ms: seconds_as_ms(message, "average="),
            ..empty
        });
    }
    if message.starts_with(CHECKPOINT_TOO_FREQUENT) {
        return Some(CheckpointEvent {
            phase: CheckpointPhase::TooFrequent,
            reason: bounded(message),
            seconds_apart: parenthesized_i64(message),
            ..empty
        });
    }
    None
}

fn parse_autovacuum(message: &str, ts: i64) -> Option<AutovacuumEvent> {
    let kind = AUTOVACUUM_PREFIXES
        .iter()
        .find_map(|(prefix, kind)| message.starts_with(prefix).then_some(*kind))?;
    let pages = tail_after(message, "pages: ");
    let tuples = tail_after(message, "tuples: ");
    let is_vacuum = kind == AutovacuumKind::Vacuum;
    let wal = message.contains("WAL usage:");
    Some(AutovacuumEvent {
        ts,
        kind,
        relation: quoted(message),
        index_scans: i64_after(message, "index scans: "),
        pages_removed: pages.and_then(i64_prefix),
        pages_remaining: pages.and_then(|tail| i64_after(tail, " removed, ")),
        tuples_removed: is_vacuum.then(|| tuples.and_then(i64_prefix)).flatten(),
        tuples_remaining: is_vacuum
            .then(|| tuples.and_then(|tail| i64_after(tail, " removed, ")))
            .flatten(),
        tuples_dead_not_removable: tuples.and_then(|tail| i64_after(tail, " remain, ")),
        elapsed_ms: seconds_as_ms(message, "elapsed: "),
        buffer_hits: i64_after(message, "buffer usage: "),
        buffer_misses: i64_after(message, " hits, "),
        buffer_dirtied: i64_after(message, " misses, ").or_else(|| i64_after(message, " reads, ")),
        avg_read_rate_mbs: f64_after(message, "avg read rate: "),
        avg_write_rate_mbs: f64_after(message, "avg write rate: "),
        cpu_user_ms: cpu_seconds_as_ms(message, "user: "),
        cpu_system_ms: cpu_seconds_as_ms(message, "system: "),
        wal_records: wal.then(|| i64_after(message, "WAL usage: ")).flatten(),
        wal_fpi: wal.then(|| i64_after(message, " records, ")).flatten(),
        wal_bytes: wal.then(|| i64_after(message, " images, ")).flatten(),
    })
}

/// Parses statement and execute duration records. Parse and bind durations are
/// excluded.
fn parse_slow_query(message: &str) -> Option<(f64, String)> {
    let rest = message.strip_prefix(DURATION_PREFIX)?;
    let (at, sql_at) = if let Some(at) = rest.find(STATEMENT_MARKER) {
        (at, at + STATEMENT_MARKER.len())
    } else {
        let at = rest.find(EXECUTE_MARKER)?;
        let named = rest.get(at + EXECUTE_MARKER.len()..)?;
        let colon = named.find(": ")?;
        (at, at + EXECUTE_MARKER.len() + colon + ": ".len())
    };
    let duration_ms = rest.get(..at)?.parse::<f64>().ok()?;
    if !duration_ms.is_finite() || duration_ms < 0.0 {
        return None;
    }
    let sql = rest.get(sql_at..)?.trim();
    Some((duration_ms, truncate(sql, MAX_TEXT_BYTES).to_owned()))
}

/// `process 123 still waiting for ShareLock on transaction 456 after 1000.1 ms`.
fn parse_lock_wait(message: &str, ts: i64) -> Option<LockWait> {
    let rest = message.strip_prefix("process ")?;
    let pid = i32_prefix(rest);
    let after_pid = rest.get(rest.find(|c: char| !c.is_ascii_digit())?..)?;
    let (kind, tail) = if let Some(tail) = after_pid.strip_prefix(" still waiting for ") {
        (LockWaitKind::Waiting, tail)
    } else if let Some(tail) = after_pid.strip_prefix(" acquired ") {
        (LockWaitKind::Acquired, tail)
    } else {
        return None;
    };
    let at = tail.find(" on ")?;
    let duration_at = tail.rfind(" after ")?;
    Some(LockWait {
        ts,
        kind,
        pid,
        lock_mode: bounded(tail.get(..at)?),
        lock_target: bounded(tail.get(at + " on ".len()..duration_at)?),
        duration_ms: duration_after(tail, " after "),
        detail: None,
        context: None,
        statement: None,
    })
}

/// Reads the PID list after a singular or plural holder marker, up to the next
/// period.
fn detail_list<'a>(detail: &'a str, marker: &str) -> Option<&'a str> {
    let at = detail.find(marker)?;
    let rest = detail.get(at + marker.len()..)?;
    let end = rest.find('.').unwrap_or(rest.len());
    let value = rest.get(..end)?.trim();
    (!value.is_empty()).then_some(value)
}

/// `temporary file: path "base/pgsql_tmp/pgsql_tmp1.0", size 1048576`.
fn parse_temp_file(message: &str, ts: i64) -> Option<TempFile> {
    if !message.starts_with(TEMP_FILE_PREFIX) {
        return None;
    }
    Some(TempFile {
        ts,
        path: quoted(message),
        size_bytes: i64_after(message, "size ").filter(|size| *size >= 0)?,
        statement: None,
    })
}

fn parse_lifecycle(message: &str, ts: i64) -> Option<LifecycleEvent> {
    let empty = LifecycleEvent {
        ts,
        kind: LifecycleKind::Crash,
        pid: None,
        signal: None,
        shutdown_mode: None,
        message: truncate(message, MAX_TEXT_BYTES).to_owned(),
        query_detail: None,
    };
    if message.starts_with(CRASH_PREFIX) {
        return Some(LifecycleEvent {
            pid: message
                .find("(PID ")
                .and_then(|at| message.get(at + "(PID ".len()..))
                .and_then(i32_prefix),
            signal: message
                .find("signal ")
                .and_then(|at| message.get(at + "signal ".len()..))
                .and_then(i32_prefix),
            ..empty
        });
    }
    for &(request, mode) in SHUTDOWN_REQUESTS {
        if message.starts_with(request) {
            return Some(LifecycleEvent {
                kind: LifecycleKind::Shutdown,
                shutdown_mode: Some(mode.to_owned()),
                ..empty
            });
        }
    }
    message
        .starts_with(READY_MESSAGE)
        .then_some(LifecycleEvent {
            kind: LifecycleKind::Ready,
            ..empty
        })
}

/// The statement a crash `DETAIL` names, empty when it names none.
fn crash_statement(detail: &str) -> Option<String> {
    if let Some(at) = detail.find("was running: ") {
        return bounded(detail.get(at + "was running: ".len()..)?);
    }
    detail.contains("was running").then(String::new)
}

/// `... 0 WAL file(s) added, 1 removed, 2 recycled`.
fn wal_file_counts(message: &str) -> (Option<i64>, Option<i64>, Option<i64>) {
    let Some(at) = message.find(" WAL file") else {
        return (None, None, None);
    };
    (
        message.get(..at).and_then(trailing_i64),
        i64_after(message, "added, "),
        i64_after(message, "removed, "),
    )
}

fn tail_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.find(marker)
        .and_then(|at| text.get(at + marker.len()..))
}

fn i64_after(text: &str, marker: &str) -> Option<i64> {
    tail_after(text, marker).and_then(i64_prefix)
}

fn i64_prefix(text: &str) -> Option<i64> {
    let end = text
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(text.len());
    text.get(..end)?.parse().ok()
}

fn i32_prefix(text: &str) -> Option<i32> {
    let end = text
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(text.len());
    text.get(..end)?.parse().ok()
}

fn trailing_i64(text: &str) -> Option<i64> {
    let trimmed = text.trim_end();
    let start = trimmed
        .rfind(|c: char| !c.is_ascii_digit() && c != '-')
        .map_or(0, |at| at + 1);
    trimmed.get(start..)?.parse().ok()
}

fn f64_after(text: &str, marker: &str) -> Option<f64> {
    let rest = tail_after(text, marker)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    let value = rest.get(..end)?.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

fn seconds_as_ms(text: &str, marker: &str) -> Option<f64> {
    f64_after(text, marker).map(|seconds| seconds * 1000.0)
}

/// Autovacuum prints `user:` and `system:` only inside its `CPU:` group.
fn cpu_seconds_as_ms(text: &str, marker: &str) -> Option<f64> {
    let cpu = text.find("CPU:").and_then(|at| text.get(at..))?;
    seconds_as_ms(cpu, marker)
}

/// The last occurrence of `marker`, so a statement quoting it cannot win.
fn duration_after(text: &str, marker: &str) -> Option<f64> {
    let rest = text.get(text.rfind(marker)? + marker.len()..)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let value = rest.get(..end)?.parse::<f64>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn parenthesized_i64(message: &str) -> Option<i64> {
    i64_prefix(message.get(message.find('(')? + 1..)?)
}

fn quoted(text: &str) -> Option<String> {
    let rest = text.get(text.find('"')? + 1..)?;
    bounded(rest.get(..rest.find('"')?)?)
}

#[cfg(test)]
mod tests;
