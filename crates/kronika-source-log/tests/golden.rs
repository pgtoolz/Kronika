//! What the parsers make of a real log file.
//!
//! The fixtures under `tests/fixtures` are log files as `PostgreSQL` and
//! `PgBouncer` write them. Each ends with a line the collector does not record,
//! which closes the record before it: a record still open when a read reaches
//! the end of the file waits for whatever might continue it.

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use chrono as _;
use memchr as _;
use serde_json as _;
use std::path::PathBuf;
use tempfile as _;

use kronika_source_log::Position;
use kronika_source_log::pgbouncer::{Level, PgBouncerLog};
use kronika_source_log::postgres::{
    AutovacuumKind, CheckpointPhase, ErrorCategory, Events, Format, LifecycleKind, LinePrefix,
    LockWaitKind, PgLog, Severity,
};

/// A timestamp for records that carry none; every fixture carries its own.
const NOW: i64 = 1_780_000_000_000_000;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str, prefix: Option<LinePrefix>) -> Events {
    let mut log = PgLog::new(fixture(name), Position::default(), prefix);
    let batch = log.read_batch(NOW, 1024).expect("read the fixture");
    if batch.needs_ack {
        log.acknowledge().expect("acknowledge the fixture");
    }
    batch.events
}

#[test]
fn the_format_follows_the_name_postgresql_writes() {
    assert_eq!(
        Format::of(&fixture("postgresql-stderr.log")),
        Format::Stderr
    );
    assert_eq!(
        Format::of(&fixture("postgresql-csvlog.csv")),
        Format::Csvlog
    );
    assert_eq!(
        Format::of(&fixture("postgresql-jsonlog.json")),
        Format::Jsonlog
    );
}

#[test]
fn a_stderr_log_yields_every_shape_it_carries() {
    let events = read(
        "postgresql-stderr.log",
        Some(LinePrefix::parse("%m [%p] %q%u@%d ")),
    );

    assert_eq!(events.errors.len(), 1, "both errors share one pattern");
    let error = &events.errors[0];
    assert_eq!(error.count, 2);
    assert_eq!(error.severity, Severity::Error);
    assert_eq!(error.category, ErrorCategory::Syntax);
    assert_eq!(error.pattern, "relation \"...\" does not exist");
    assert_eq!(error.username.as_deref(), Some("alice"));
    assert_eq!(error.database.as_deref(), Some("shop"));
    assert_eq!(error.statement.as_deref(), Some("select * from orders"));

    assert_eq!(events.checkpoints.len(), 2);
    assert_eq!(events.checkpoints[0].phase, CheckpointPhase::Starting);
    assert_eq!(events.checkpoints[0].reason.as_deref(), Some("wal"));
    assert_eq!(events.checkpoints[1].phase, CheckpointPhase::Complete);
    assert_eq!(events.checkpoints[1].buffers_written, Some(3));

    assert_eq!(events.slow_queries.len(), 1);
    assert!(
        (events.slow_queries[0].max_duration_ms - 1234.567).abs() < 1e-9,
        "the slowest occurrence keeps its duration"
    );
    assert_eq!(events.slow_queries[0].sample, "select pg_sleep(1)");

    assert_eq!(events.lock_waits.len(), 1);
    let wait = &events.lock_waits[0];
    assert_eq!(wait.kind, LockWaitKind::Waiting);
    assert_eq!(wait.pid, Some(12_348));
    assert_eq!(wait.lock_target.as_deref(), Some("transaction 987"));
    assert_eq!(wait.holding_pids(), Some("12347"));
    assert_eq!(wait.wait_queue(), Some("12348"));
    let detail = wait.detail.as_deref().expect("lock detail");
    for borrowed in [wait.holding_pids(), wait.wait_queue()]
        .into_iter()
        .flatten()
    {
        let offset = borrowed.as_ptr() as usize - detail.as_ptr() as usize;
        assert!(offset < detail.len(), "participant list borrows DETAIL");
    }
    assert_eq!(
        wait.detail.as_deref(),
        Some("Process holding the lock: 12347. Wait queue: 12348.")
    );
    assert_eq!(
        wait.statement.as_deref(),
        Some("update orders set total = 1 where id = 1")
    );

    assert_eq!(events.temp_files.len(), 1);
    assert_eq!(events.temp_files[0].size_bytes, 1_048_576);
    assert_eq!(
        events.temp_files[0].statement.as_deref(),
        Some("select * from big order by 1")
    );

    assert_eq!(events.lifecycle.len(), 2);
    assert_eq!(events.lifecycle[0].kind, LifecycleKind::Crash);
    assert_eq!(events.lifecycle[0].signal, Some(9));
    assert_eq!(
        events.lifecycle[0].query_detail.as_deref(),
        Some("select count(*) from huge")
    );
    assert_eq!(events.lifecycle[1].kind, LifecycleKind::Ready);

    assert_eq!(events.autovacuum.len(), 1);
    let vacuum = &events.autovacuum[0];
    assert_eq!(vacuum.kind, AutovacuumKind::Vacuum);
    assert_eq!(vacuum.relation.as_deref(), Some("shop.public.orders"));
    assert_eq!(vacuum.tuples_removed, Some(100));
    assert_eq!(vacuum.wal_bytes, Some(4567));
}

#[test]
fn a_stderr_log_read_without_the_prefix_keeps_its_events() {
    let events = read("postgresql-stderr.log", None);

    assert_eq!(events.errors.len(), 1);
    assert_eq!(events.errors[0].count, 2);
    assert_eq!(
        events.errors[0].database, None,
        "the database is only in the prefix"
    );
    assert_eq!(events.checkpoints.len(), 2);
}

#[test]
fn a_csvlog_carries_the_database_and_a_statement_with_newlines_in_it() {
    let events = read("postgresql-csvlog.csv", None);

    assert_eq!(events.errors.len(), 1);
    let error = &events.errors[0];
    assert_eq!(error.count, 2);
    assert_eq!(error.sqlstate.as_deref(), Some("42P01"));
    assert_eq!(error.database.as_deref(), Some("shop"));
    assert_eq!(error.username.as_deref(), Some("alice"));
    assert_eq!(error.statement.as_deref(), Some("select * from orders"));

    assert_eq!(events.checkpoints.len(), 1);
    assert_eq!(events.checkpoints[0].total_ms, Some(230.0));
    assert_eq!(events.slow_queries.len(), 1);
    assert_eq!(events.slow_queries[0].count, 1);
}

#[test]
fn a_jsonlog_yields_the_same_events_as_the_csvlog_of_the_same_records() {
    let events = read("postgresql-jsonlog.json", None);

    assert_eq!(events.errors.len(), 1);
    let error = &events.errors[0];
    assert_eq!(error.count, 2);
    assert_eq!(error.sqlstate.as_deref(), Some("42P01"));
    assert_eq!(error.database.as_deref(), Some("shop"));
    assert_eq!(error.statement.as_deref(), Some("select * from orders"));

    assert_eq!(events.checkpoints.len(), 1);
    assert_eq!(events.checkpoints[0].total_ms, Some(230.0));
    assert_eq!(events.slow_queries.len(), 1);
}

#[test]
fn a_pgbouncer_log_yields_one_row_per_event_and_no_duplicates() {
    let mut log = PgBouncerLog::new(fixture("pgbouncer.log"), Position::default());

    let batch = log.read_batch(1024).expect("read the fixture");
    if batch.needs_ack {
        log.acknowledge().expect("acknowledge the fixture");
    }
    let events = batch.events;

    let texts: Vec<&str> = events.iter().map(|event| event.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "query_wait_timeout",
            "server conn crashed?",
            "no such database: nope",
            "server login failed: FATAL password authentication failed for user \"alice\"",
            "kernel file descriptor limit: 1024 (hard: 4096); max_client_conn: 100, max expected fd use: 172",
            "bad packet",
        ]
    );

    assert_eq!(events[0].level, Level::Log);
    assert_eq!(events[0].database.as_deref(), Some("shop"));
    assert_eq!(events[0].username.as_deref(), Some("alice"));
    assert_eq!(events[0].host.as_deref(), Some("10.0.0.1"));

    assert_eq!(events[2].database.as_deref(), Some("(nodb)"));
    assert_eq!(events[2].username.as_deref(), Some("(nouser)"));
    assert_eq!(events[2].host.as_deref(), Some("unix(9990)"));

    assert_eq!(events[3].level, Level::Warning);
    assert_eq!(events[4].host, None, "a janitor line carries no socket");
    assert_eq!(events[5].host.as_deref(), Some("[2001:db8::1]"));
}
