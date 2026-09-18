//! Parse `PostgreSQL`'s event-specific message fields, retaining their original units.

use super::{
    AutovacuumEvent, AutovacuumKind, CheckpointEvent, CheckpointPhase, LifecycleEvent,
    LifecycleKind, LockWait, LockWaitKind, TempFile,
};
use crate::text::{MAX_TEXT_BYTES, bounded, truncate};

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

pub(super) fn parse_checkpoint(message: &str, ts: i64) -> Option<CheckpointEvent> {
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

pub(super) fn parse_autovacuum(message: &str, ts: i64) -> Option<AutovacuumEvent> {
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
        pages_removed: pages.and_then(integer_prefix),
        pages_remaining: pages.and_then(|tail| i64_after(tail, " removed, ")),
        tuples_removed: is_vacuum.then(|| tuples.and_then(integer_prefix)).flatten(),
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
pub(super) fn parse_slow_query(message: &str) -> Option<(f64, String)> {
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
pub(super) fn parse_lock_wait(message: &str, ts: i64) -> Option<LockWait> {
    let rest = message.strip_prefix("process ")?;
    let pid = integer_prefix(rest);
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
pub(super) fn detail_list<'a>(detail: &'a str, marker: &str) -> Option<&'a str> {
    let at = detail.find(marker)?;
    let rest = detail.get(at + marker.len()..)?;
    let end = rest.find('.').unwrap_or(rest.len());
    let value = rest.get(..end)?.trim();
    (!value.is_empty()).then_some(value)
}

/// `temporary file: path "base/pgsql_tmp/pgsql_tmp1.0", size 1048576`.
pub(super) fn parse_temp_file(message: &str, ts: i64) -> Option<TempFile> {
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

pub(super) fn parse_lifecycle(message: &str, ts: i64) -> Option<LifecycleEvent> {
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
                .and_then(integer_prefix),
            signal: message
                .find("signal ")
                .and_then(|at| message.get(at + "signal ".len()..))
                .and_then(integer_prefix),
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
pub(super) fn crash_statement(detail: &str) -> Option<String> {
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
    text.split_once(marker).map(|(_head, tail)| tail)
}

fn i64_after(text: &str, marker: &str) -> Option<i64> {
    tail_after(text, marker).and_then(integer_prefix)
}

fn integer_prefix<T: std::str::FromStr>(text: &str) -> Option<T> {
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
    integer_prefix(message.get(message.find('(')? + 1..)?)
}

fn quoted(text: &str) -> Option<String> {
    let rest = text.get(text.find('"')? + 1..)?;
    bounded(rest.get(..rest.find('"')?)?)
}
