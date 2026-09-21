//! What the parsers make of a real log file.
//!
// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use chrono as _;
use jiff as _;
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

/// Read time for parser fixtures.
const NOW: i64 = 1_780_000_000_000_000;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str, prefix: Option<LinePrefix>) -> Events {
    let mut log = PgLog::new(fixture(name), Position::default(), prefix);
    log.set_timezone(
        kronika_source_log::postgres::LogTimezone::parse("Europe/Moscow").expect("zone"),
    );
    let batch = log
        .read_batch(|| Ok(NOW), 1024, 900)
        .expect("read the fixture");
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
fn a_pgbouncer_log_retains_full_messages_and_connection_context() {
    let mut log = PgBouncerLog::new(fixture("pgbouncer.log"), Position::default());

    let batch = log.read_batch(|| Ok(NOW), 1024).expect("read the fixture");
    if batch.needs_ack {
        log.acknowledge().expect("acknowledge the fixture");
    }
    let events = batch.events;

    let texts: Vec<&str> = events.iter().map(|event| event.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "closing because: query_wait_timeout (age=42s)",
            "pooler error: query_wait_timeout",
            "closing because: server conn crashed? (age=10s)",
            "closing because: no such database: nope (age=0s)",
            "server login failed: FATAL password authentication failed for user \"alice\"",
            "kernel file descriptor limit: 1024 (hard: 4096); max_client_conn: 100, max expected fd use: 172",
            "closing because: bad packet (age=1s)",
        ]
    );

    assert_eq!(events[0].level, Level::Log);
    assert_eq!(events[0].database.as_deref(), Some("shop"));
    assert_eq!(events[0].username.as_deref(), Some("alice"));
    assert_eq!(events[0].host.as_deref(), Some("10.0.0.1"));

    assert_eq!(events[3].database.as_deref(), Some("(nodb)"));
    assert_eq!(events[3].username.as_deref(), Some("(nouser)"));
    assert_eq!(events[3].host.as_deref(), Some("unix(9990)"));

    assert_eq!(events[4].level, Level::Warning);
    assert_eq!(events[5].host, None, "a janitor line carries no socket");
    assert_eq!(events[6].host.as_deref(), Some("[2001:db8::1]"));
}

#[test]
fn all_postgres_formats_resolve_gmt_before_classifying_events() {
    for (name, content, prefix) in [
        (
            "case.log",
            "2026-09-14 10:13:00.789 GMT [1] ERROR:  test error\n",
            Some(LinePrefix::parse("%m [%p] ")),
        ),
        (
            "case.csv",
            "2026-09-14 10:13:00.789 GMT,alice,shop,1,,session,1,SELECT,,3/1,0,ERROR,42P01,test error,,,,,,select 1,0,,psql\n",
            None,
        ),
        (
            "case.json",
            "{\"timestamp\":\"2026-09-14 10:13:00.789 GMT\",\"error_severity\":\"ERROR\",\"message\":\"test error\"}\n",
            None,
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        std::fs::write(&path, format!("{content}ignored\n")).expect("fixture");
        let mut log = PgLog::new(path, Position::default(), prefix);
        let batch = log.read_batch(|| Ok(NOW), 1024, 900).expect("parse");
        assert_eq!(batch.events.errors[0].ts, 1_789_380_780_789_000, "{name}");
    }
}

#[test]
fn missing_or_invalid_time_uses_batch_time_and_does_not_block_any_format() {
    for extension in ["log", "csv", "json"] {
        let line = |timestamp: Option<&str>, message: &str| match extension {
            "csv" => {
                let mut fields = [""; 23];
                fields[0] = timestamp.unwrap_or_default();
                fields[11] = "ERROR";
                fields[13] = message;
                format!("{}\n", fields.join(","))
            }
            "json" => format!(
                "{}\n",
                serde_json::json!({"timestamp":timestamp,"error_severity":"ERROR","message":message})
            ),
            _ => format!(
                "{}ERROR:  {message}\n",
                timestamp.map_or(String::new(), |ts| format!("{ts} "))
            ),
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(format!("fallback.{extension}"));
        let input = format!(
            "{}{}{}garbage\n{}ignored\n",
            line(None, "missing timestamp"),
            line(Some("broken"), "invalid timestamp"),
            line(Some("2026-09-14 10:13:00 XYZ"), "unknown timezone"),
            line(Some("2026-09-14 10:13:00.789 GMT"), "valid successor")
        );
        std::fs::write(&path, &input).expect("write fixture");
        let mut log = PgLog::new(path, Position::default(), Some(LinePrefix::parse("%m ")));
        let mut rows = Vec::new();
        for _ in 0..8 {
            let batch = log
                .read_batch(|| Ok(NOW), 1, 900)
                .expect("bounded content progress");
            rows.extend(batch.events.errors);
            if batch.needs_ack {
                log.acknowledge().expect("ack");
            }
        }
        assert_eq!(rows.len(), 4, "{extension}");
        assert!(rows[..3].iter().all(|row| row.ts == NOW));
        assert_eq!(rows[3].ts, 1_789_380_780_789_000);
        assert_eq!(rows[3].sample, "valid successor");
        assert_eq!(log.position().offset, input.len() as u64);
    }
}

#[test]
fn conditional_prefix_keeps_valid_time_and_falls_back_when_missing_or_invalid() {
    for (head, expected) in [
        ("[123]", NOW),
        ("[123] 2026-09-14 10:13:00.789 GMT ", 1_789_380_780_789_000),
        ("[123] broken ", NOW),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("conditional.log");
        std::fs::write(
            &path,
            format!("{head}LOG:  checkpoint starting: time\nignored\n"),
        )
        .expect("fixture");
        let mut log = PgLog::new(
            path,
            Position::default(),
            Some(LinePrefix::parse("[%p]%q %m ")),
        );
        let batch = log
            .read_batch(|| Ok(NOW), 1024, 900)
            .expect("usable record");
        assert_eq!(batch.events.checkpoints[0].ts, expected);
        assert!(log.acknowledge().is_some());
    }
}

#[test]
fn a_colon_after_the_timezone_does_not_block_a_postgres_batch() {
    for (timezone, clock, label, prefix, suffix) in [
        ("GMT", "10:13:00", "GMT", "%t: [%p] ", ": [1] "),
        ("Etc/GMT-3", "13:13:00", "+03", "%t: [%p] ", ": [1] "),
        ("Etc/GMT-3", "13:13:00", "+03", "%t:%p ", ":34 "),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("colon.log");
        std::fs::write(
            &path,
            format!("2026-09-14 {clock} {label}{suffix}ERROR:  test error\nignored\n"),
        )
        .expect("fixture");
        let mut log = PgLog::new(path, Position::default(), Some(LinePrefix::parse(prefix)));
        log.set_timezone(kronika_source_log::postgres::LogTimezone::parse(timezone).expect("zone"));
        let batch = log.read_batch(|| Ok(NOW), 1024, 900).expect("valid prefix");
        assert_eq!(batch.events.errors[0].ts, 1_789_380_780_000_000);
        assert!(log.acknowledge().is_some());
    }
}

#[test]
fn max_lag_filters_records_before_grouping_in_every_pg_format() {
    const READ_AT: i64 = 1_789_380_000_000_000; // 2026-09-14 10:00 UTC.
    for extension in ["log", "csv", "json"] {
        let line = |clock: &str, severity: &str| {
            let timestamp = format!("2026-09-14 {clock} GMT");
            match extension {
                "csv" => {
                    let mut fields = [""; 23];
                    fields[0] = &timestamp;
                    fields[11] = severity;
                    fields[13] = "test error";
                    format!("{}\n", fields.join(","))
                }
                "json" => format!(
                    "{}\n",
                    serde_json::json!({"timestamp": timestamp, "error_severity": severity, "message": "test error"})
                ),
                _ => format!("{timestamp} {severity}:  test error\n"),
            }
        };
        for (clocks, count) in [
            (vec!["09:44:59.999999"], 0),
            (
                vec!["09:44:59.999999", "09:45:00", "10:00:00", "14:00:00"],
                3,
            ),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join(format!("postgresql.{extension}"));
            let records: String = clocks.iter().map(|clock| line(clock, "ERROR")).collect();
            std::fs::write(&path, format!("{records}{}", line("14:00:01", "INFO")))
                .expect("write log");
            let mut log = PgLog::new(
                path.clone(),
                Position::default(),
                Some(LinePrefix::parse("%m ")),
            );
            let mut clock_reads = 0;
            let batch = log
                .read_batch(
                    || {
                        clock_reads += 1;
                        Ok(READ_AT)
                    },
                    1024,
                    900,
                )
                .expect("filter log");
            assert_eq!(clock_reads, 1);
            assert_eq!(
                batch.events.errors.iter().map(|row| row.count).sum::<u32>(),
                count
            );
            if count != 0 {
                assert_eq!(batch.events.errors[0].ts, READ_AT - 900_000_000);
            }
            assert!(batch.needs_ack);
            assert_eq!(log.position().offset, 0);
            let committed = log
                .acknowledge()
                .expect("acknowledge accepted or skipped rows");
            assert!(committed.offset >= records.len() as u64);
            let mut resumed = PgLog::new(path, committed, Some(LinePrefix::parse("%m ")));
            assert!(
                resumed
                    .read_batch(|| Ok(READ_AT), 1024, 900)
                    .expect("resume")
                    .events
                    .is_empty()
            );
        }
    }
}

#[test]
fn raw_crash_notice_does_not_block_following_records() {
    const WARNING: &str = "terminating connection because of crash of another server process";
    const DETAIL: &str = "The postmaster has commanded this server process to roll back the current transaction and exit, because another server process exited abnormally and possibly corrupted shared memory.";
    const HINT: &str =
        "In a moment you should be able to reconnect to the database and repeat your command.";
    for preceding in [
        "2026-09-21 05:20:22.594 GMT [1608537] postgres [unknown] postgres 127.0.0.1 6a8d266c.188b59 LOG:  duration: 3965.910 ms  statement:\n    SELECT *,\n     extract(epoch from now() - last_archived_time) AS last_archive_age\n    FROM pg_stat_archiver\n    \n",
        "2026-09-21 05:20:22.594 GMT [1608537] postgres [unknown] postgres 127.0.0.1 6a8d266c.188b59 LOG:  duration: 3965.910 ms  statement: SELECT 1\n",
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("crash.log");
        let complete = format!(
            "{preceding}WARNING:  {WARNING}\nDETAIL:  {DETAIL}\nHINT:  {HINT}\n2026-09-21 05:20:23.834 GMT [1608447]     6a8d264c.188aff FATAL:  could not receive data from WAL stream: server closed the connection unexpectedly\n2026-09-21 05:20:24.000 GMT [1]     session LOG:  checkpoint starting: time\n"
        );
        std::fs::write(&path, format!("{complete}ignored\n")).expect("write crash fixture");
        let mut log = PgLog::new(
            path.clone(),
            Position::default(),
            Some(LinePrefix::parse("%m [%p] %u %a %d %h %c ")),
        );
        log.set_timezone(kronika_source_log::postgres::LogTimezone::parse("GMT").expect("zone"));
        let batch = log
            .read_batch(|| Ok(NOW), 1024, 900)
            .expect("content must not stop collection");
        let warning = batch
            .events
            .errors
            .iter()
            .find(|row| row.severity == Severity::Warning)
            .expect("raw warning retained");
        assert_eq!(warning.ts, NOW);
        assert_eq!(warning.sample, WARNING);
        assert_eq!(warning.detail.as_deref(), Some(DETAIL));
        assert_eq!(warning.hint.as_deref(), Some(HINT));
        assert_eq!(warning.statement, None);
        assert_eq!(warning.database, None);
        assert_eq!(warning.username, None);
        assert_eq!(
            batch
                .events
                .errors
                .iter()
                .filter(|row| row.severity == Severity::Fatal)
                .count(),
            1
        );
        assert_eq!(batch.events.checkpoints.len(), 1);
        assert!(
            batch
                .events
                .slow_queries
                .iter()
                .all(|row| !row.sample.contains(WARNING))
        );
        assert_eq!(log.position().offset, 0);
        let position = log.acknowledge().expect("ack");
        assert_eq!(position.offset, (complete.len() + "ignored\n".len()) as u64);
        let mut restarted = PgLog::new(
            path,
            position,
            Some(LinePrefix::parse("%m [%p] %u %a %d %h %c ")),
        );
        assert!(
            restarted
                .read_batch(|| Ok(NOW), 1024, 900)
                .expect("restart")
                .events
                .is_empty()
        );
    }
}

#[test]
fn bare_warning_waits_for_complete_input_then_keeps_present_details() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("partial.log");
    std::fs::write(&path, "WARNING:  raw warning").expect("partial write");
    let prefix = Some(LinePrefix::parse(
        "%t [%p] => [%l-1] client=%h,db=%d,user=%u ",
    ));
    let mut log = PgLog::new(path.clone(), Position::default(), prefix);
    for _ in 0..2 {
        let batch = log.read_batch(|| Ok(NOW), 1, 900).expect("partial read");
        assert!(batch.events.is_empty());
        assert!(!batch.needs_ack);
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append");
    file.write_all(b"\nDETAIL:  detail body\nHINT:  hint body\n2026-09-14 10:13:00 GMT [1] => [1-1] client=,db=,user= LOG:  database system is ready to accept connections\n").expect("complete input");
    let batch = log
        .read_batch(|| Ok(NOW), 1, 900)
        .expect("complete warning");
    assert_eq!(batch.events.errors.len(), 1);
    let warning = &batch.events.errors[0];
    assert_eq!(warning.ts, NOW);
    assert_eq!(warning.sample, "raw warning");
    assert_eq!(warning.detail.as_deref(), Some("detail body"));
    assert_eq!(warning.hint.as_deref(), Some("hint body"));
    log.acknowledge().expect("ack warning");
    let batch = log.read_batch(|| Ok(NOW), 1, 900).expect("flush lifecycle");
    assert_eq!(batch.events.lifecycle.len(), 1);
    log.acknowledge().expect("ack lifecycle");
    assert_eq!(
        log.position().offset,
        std::fs::metadata(path).expect("metadata").len()
    );
}

#[test]
fn a_new_severity_quoting_a_detail_marker_starts_its_own_record() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("postgresql.log");
    let input = "LOG:  duration: 1 ms  statement: SELECT 1\n\t-- WARNING:  quoted wrapped SQL\nWARNING:  message quoting DETAIL:  text\nDETAIL:  quoted WARNING:  is still detail\nHINT:  reconnect\nFATAL:  following error\nINFO:  sentinel\n";
    std::fs::write(&path, input).expect("write log");
    let mut log = PgLog::new(path, Position::default(), Some(LinePrefix::parse("%m ")));
    let batch = log.read_batch(|| Ok(NOW), 8, 900).expect("read records");
    assert_eq!(batch.events.slow_queries.len(), 1);
    assert_eq!(batch.events.errors.len(), 2);
    let warning = batch
        .events
        .errors
        .iter()
        .find(|row| row.severity == Severity::Warning)
        .expect("new warning");
    assert_eq!(warning.sample, "message quoting DETAIL:  text");
    assert_eq!(
        warning.detail.as_deref(),
        Some("quoted WARNING:  is still detail")
    );
    assert_eq!(warning.hint.as_deref(), Some("reconnect"));
    assert_eq!(warning.statement, None);
    assert_eq!(warning.ts, NOW);
    log.acknowledge().expect("commit complete records");
    assert_eq!(log.position().offset, input.len() as u64);
}

#[test]
fn pgbouncer_mixed_batch_preserves_messages_and_retries_unacknowledged_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pooler.log");
    std::fs::write(&path, concat!(
        "garbage\n",
        "WARNING message contains [12] DEBUG nested text\n",
        "bad-time [21] FATAL previously unseen fatal\n",
        "2026-09-21 06:00:00.000 [22] ERROR C-0x1: db/alice@[::1]:6432 closing because: new failure (age=42s)\n",
        "WARNING S-0x2: db/bob@::1 pooler error: independent warning\n",
        "LOG S-0x3: db/bob@unix(9990):6432 closing because: client unexpected eof (age=0s)\n",
        "LOG got SIGTERM, shutting down\n",
        "LOG reloading config file\n",
        "LOG new connection to server failed\n",
        "LOG closing because: server idle timeout (age=bad)\n",
        "WARNING closing because: client close request (age=1s)\n",
        "WARNING C-broken context remains visible\n",
        "LOG login attempt: db=db user=alice tls=no\n",
        "LOG new connection to server\n",
        "LOG new connection to server (from [::1]:4000)\n",
        "LOG closing because: client close request (age=10s)\n",
        "LOG closing because: server idle timeout (age=10s)\n",
        "LOG closing because: server lifetime over (age=10s)\n",
        "LOG stats: 1 xacts/s, 2 queries/s, in 3 B/s, out 4 B/s, xact 5 us, query 6 us, wait 7 us\n",
        "LOG closing because: client close request (age=+10s)\n",
        "DEBUG hidden\nNOISE hidden\ngarbage sentinel\n",
    )).expect("write log");
    let mut log = PgBouncerLog::new(path.clone(), Position::default());
    assert!(
        log.read_batch(|| Err(std::io::Error::other("clock unavailable")), 100)
            .is_err()
    );
    assert_eq!(log.position().offset, 0);
    let calls = std::cell::Cell::new(0);
    let batch = log
        .read_batch(
            || {
                calls.set(calls.get() + 1);
                Ok(NOW)
            },
            100,
        )
        .expect("read");
    assert_eq!(calls.get(), 1);
    let events = &batch.events;
    assert_eq!(events.len(), 12);
    assert_eq!(events[0].text, "message contains [12] DEBUG nested text");
    assert_eq!(events[0].level, Level::Warning);
    assert_eq!(events[0].pid, None);
    assert_eq!(events[1].pid, Some(21));
    assert_eq!(events[2].pid, Some(22));
    assert_eq!(events[2].side.as_deref(), Some("C"));
    assert_eq!(events[2].host.as_deref(), Some("[::1]"));
    assert_eq!(events[2].port, Some(6432));
    assert_eq!(events[2].age_s, Some(42));
    assert_eq!(events[2].text, "closing because: new failure (age=42s)");
    assert_ne!(events[2].ts, NOW);
    assert!(
        events
            .iter()
            .enumerate()
            .all(|(index, event)| index == 2 || event.ts == NOW)
    );
    assert_eq!(events[3].side.as_deref(), Some("S"));
    assert_eq!(events[3].host.as_deref(), Some("::1"));
    assert_eq!(events[3].port, None);
    assert_eq!(events[4].host.as_deref(), Some("unix(9990)"));
    assert_eq!(events[4].port, Some(6432));
    assert_eq!(events[4].age_s, Some(0));
    assert_eq!(events[8].age_s, None);
    assert_eq!(events[10].text, "C-broken context remains visible");
    log.retry();
    assert_eq!(
        log.read_batch(|| Ok(NOW), 100).expect("retry").events,
        *events
    );
    let position = log.acknowledge().expect("ack");
    let mut restarted = PgBouncerLog::new(path, position);
    assert!(
        restarted
            .read_batch(|| Ok(NOW), 100)
            .expect("restart")
            .events
            .is_empty()
    );
}

#[test]
fn pr12_journal_fixture_retains_native_events_and_both_error_messages() {
    let mut plain = PgBouncerLog::new(fixture("pgbouncer.log"), Position::default());
    let plain = plain
        .read_batch(|| Ok(NOW), 1024)
        .expect("plain input")
        .events;
    let path = fixture("pgbouncer-journald.log");
    let mut wrapped = PgBouncerLog::new(path.clone(), Position::default());
    let batch = wrapped.read_batch(|| Ok(NOW), 1024).expect("wrapped input");
    assert_eq!(batch.events.len(), 9);
    assert_eq!(&batch.events[..plain.len()], plain.as_slice());
    assert_eq!(
        batch.events[7].text,
        "closing because: query_timeout (age=3s)"
    );
    assert!(
        batch.events[8]
            .text
            .starts_with("process up: PgBouncer 1.16.0,")
    );
    assert_eq!(wrapped.position().offset, 0);
    let position = wrapped.acknowledge().expect("ack");
    assert_eq!(
        position.offset,
        std::fs::metadata(&path).expect("length").len()
    );
    let mut restarted = PgBouncerLog::new(path, position);
    assert!(
        restarted
            .read_batch(|| Ok(NOW), 1)
            .expect("restart")
            .events
            .is_empty()
    );
}

#[test]
fn pr12_pooler_errors_are_not_deduplicated_on_retry_or_restart() {
    let path = fixture("pgbouncer-pooler-errors.log");
    let mut log = PgBouncerLog::new(path.clone(), Position::default());
    let mut events = Vec::new();
    loop {
        let batch = log.read_batch(|| Ok(NOW), 1).expect("one record");
        assert!(batch.events.len() <= 1);
        if batch.needs_ack {
            log.retry();
            let replay = log.read_batch(|| Ok(NOW), 1).expect("retry");
            assert_eq!(replay.events, batch.events);
            events.extend(replay.events);
            let position = log.acknowledge().expect("ack after admission");
            log = PgBouncerLog::new(path.clone(), position);
        }
        if batch.at_eof {
            break;
        }
    }
    assert_eq!(
        events
            .iter()
            .map(|event| event.text.as_str())
            .collect::<Vec<_>>(),
        [
            "closing because: query_wait_timeout (age=42s)",
            "pooler error: query_wait_timeout",
            "pooler error: no such user",
            "pooler error: SSL required",
            "server login failed: FATAL database \"nope\" does not exist",
            "pooler error: database \"nope\" does not exist",
            "closing because: query_wait_timeout (age=1s)",
            "pooler error: query_wait_timeout",
            "pooler error: password authentication failed",
            "pooler error: \"trust\" authentication failed",
        ]
    );
    assert_eq!(events[0].level, Level::Log);
    assert_eq!(events[1].level, Level::Warning);
    assert_eq!(events[0].username, events[1].username);
    assert_eq!(events[3].database.as_deref(), Some("(nodb)"));
    assert_eq!(events[5].database.as_deref(), Some("nope"));
    assert_eq!(events[9].username.as_deref(), Some("grace"));
    assert_eq!(
        log.position().offset,
        std::fs::metadata(path).expect("length").len()
    );
}

#[test]
fn text_wrappers_use_only_payload_time_pid_and_multiline_text() {
    let inner = "2026-08-07 12:34:56.789 UTC [12345] WARNING C-0x1: shop/alice@[::1]:6432 failure";
    let expected_time = chrono::DateTime::parse_from_rfc3339("2026-08-07T12:34:56.789Z")
        .expect("inner time")
        .timestamp_micros();
    for prefix in [
        "Aug  7 01:02:03 host pgbouncer[762]: ",
        "Fri 2026-08-07 01:02:03 UTC pgbouncer[762]: ",
        "2026-08-07T01:02:03+0000 host pgbouncer[762]: ",
        "2026-08-07 01:02:03 UTC host pgbouncer[762]: ",
    ] {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("wrapped.log");
        let input = format!(
            "garbage\n{prefix}{inner}\n{prefix}\twrapped detail\n\tnative detail app[999]: literal\n{prefix}bad-time [66] ERROR invalid time\n{prefix}WARNING missing time\n"
        );
        std::fs::write(&path, &input).expect("mixed input");
        let mut log = PgBouncerLog::new(path.clone(), Position::default());
        let mut events = Vec::new();
        loop {
            let batch = log.read_batch(|| Ok(NOW), 1).expect("bounded wrapped read");
            if batch.needs_ack {
                log.retry();
                let replay = log
                    .read_batch(|| Ok(NOW), 1)
                    .expect("replay wrapped record");
                assert_eq!(replay.events, batch.events);
                events.extend(replay.events);
                let position = log.acknowledge().expect("ack");
                log = PgBouncerLog::new(path.clone(), position);
            }
            if batch.at_eof {
                break;
            }
        }
        assert_eq!(events.len(), 3, "{prefix}");
        assert_eq!(events[0].ts, expected_time);
        assert_eq!(events[0].pid, Some(12345));
        assert_eq!(
            events[0].text,
            "failure wrapped detail native detail app[999]: literal"
        );
        assert_eq!(events[0].host.as_deref(), Some("[::1]"));
        assert_eq!(events[0].port, Some(6432));
        assert_eq!(events[0].side.as_deref(), Some("C"));
        assert_eq!(events[1].ts, NOW);
        assert_eq!(events[1].pid, Some(66));
        assert_eq!(events[2].ts, NOW);
        assert_eq!(events[2].pid, None);
        assert_eq!(log.position().offset, input.len() as u64);
        assert!(
            log.read_batch(|| Ok(NOW), 1)
                .expect("idle")
                .events
                .is_empty()
        );
    }
}

#[test]
fn native_headers_win_over_wrapper_markers_inside_messages() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("native.log");
    let input = concat!(
        "WARNING bare app[999]: 2026-08-07 01:02:03 UTC [88] DEBUG nested\n",
        "bad-time [66] ERROR native app[999]: WARNING nested\n",
        "2026-08-07 12:34:56.789 UTC [12345] FATAL native app[999]: WARNING nested\n",
        "2026-08-07 12:34:56.789 UTC WARNING no pid app[999]: WARNING nested\n",
        "DEBUG app[999]: WARNING not an event\n",
        "bad-time [66] NOISE app[999]: WARNING not an event\n",
        "2026-08-07 12:34:56.789 UTC [12345] DEBUG app[999]: WARNING not an event\n",
        "LOG login attempt: db=shop user=alice tls=app[999]: WARNING not an event\n",
    );
    std::fs::write(&path, input).expect("native input");
    let mut log = PgBouncerLog::new(path, Position::default());
    let events = log.read_batch(|| Ok(NOW), 16).expect("read").events;
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].level, Level::Warning);
    assert_eq!(events[0].pid, None);
    assert_eq!(
        events[0].text,
        "bare app[999]: 2026-08-07 01:02:03 UTC [88] DEBUG nested"
    );
    assert_eq!(events[1].pid, Some(66));
    assert_eq!(events[1].text, "native app[999]: WARNING nested");
    assert_eq!(events[2].pid, Some(12345));
    assert_eq!(events[2].level, Level::Fatal);
    assert_eq!(events[3].pid, None);
    assert_eq!(events[3].text, "no pid app[999]: WARNING nested");
}

#[test]
fn native_journal_owner_message_preserves_outer_time_and_pid() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("journal.log");
    std::fs::write(&path, "Mon 2026-09-21 08:17:37 EDT pgbouncer[1118]: tls_sbufio_recv: read failed: Connection reset by peer\n").expect("input");
    let mut log = PgBouncerLog::new(path, Position::default());
    let events = log.read_batch(|| Ok(NOW), 8).expect("read").events;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].ts, 1_789_993_057_000_000);
    assert_eq!(events[0].pid, Some(1118));
    assert_eq!(events[0].level.code(), 3);
    assert_eq!(
        events[0].text,
        "tls_sbufio_recv: read failed: Connection reset by peer"
    );
}

#[test]
fn native_journal_formats_preserve_context_and_retry_boundaries() {
    let cases = [
        (
            "Mon 2026-09-21 08:17:37 EDT host",
            Some(1_789_993_057_000_000),
        ),
        ("2026-09-21 12:17:37 UTC", Some(1_789_993_057_000_000)),
        ("2026-09-21T08:17:37-0400 host", Some(1_789_993_057_000_000)),
        ("2026-09-21T14:17:37+02:00", Some(1_789_993_057_000_000)),
        ("2026-09-21T12:17:37Z", Some(1_789_993_057_000_000)),
        ("2026-09-21T08:17:37-04:00", Some(1_789_993_057_000_000)),
        ("2026-09-21T14:17:37+0200", Some(1_789_993_057_000_000)),
        (
            "2026-09-21 08:17:37 -04:00 host",
            Some(1_789_993_057_000_000),
        ),
        (
            "2026-09-21T12:17:37.123456Z host",
            Some(1_789_993_057_123_456),
        ),
        ("Mon 2026-09-21 08:17:37 CST host", None),
        ("Mon 2026-09-21 08:17:37 Unknown host", None),
        ("Mon 2026-09-21 08:17:37 UTC? host", None),
        ("Mon 2026-99-21 08:17:37 EDT host", None),
    ];
    for (prefix, expected) in cases {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("journal.log");
        let input = format!(
            "garbage\n{prefix} pgbouncer[1118]: S-0x1: db/alice@[::1]:6432 closing because: unexpected eof (age=42s)\n{prefix} pgbouncer[1118]: \tfull detail\n{prefix} pgbouncer[1118]: C-0x2: db/bob@127.0.0.1:4321 unfamiliar diagnostic\n{prefix} pgbouncer[1118]: closing because: client close request (age=1s)\ninvalid prefix[pid]: ignored\n"
        );
        std::fs::write(&path, &input).expect("input");
        let mut log = PgBouncerLog::new(path.clone(), Position::default());
        let batch = log.read_batch(|| Ok(NOW), 32).expect("read");
        assert_eq!(batch.events.len(), 2, "{prefix}");
        let event = &batch.events[0];
        assert_eq!(event.ts, expected.unwrap_or(NOW), "{prefix}");
        assert_eq!(event.pid, Some(1118));
        assert_eq!(event.level, Level::Log);
        assert_eq!(
            event.text,
            "closing because: unexpected eof (age=42s) full detail"
        );
        assert_eq!(event.side.as_deref(), Some("S"));
        assert_eq!(event.database.as_deref(), Some("db"));
        assert_eq!(event.username.as_deref(), Some("alice"));
        assert_eq!(event.host.as_deref(), Some("[::1]"));
        assert_eq!(event.port, Some(6432));
        assert_eq!(event.age_s, Some(42));
        assert_eq!(batch.events[1].side.as_deref(), Some("C"));
        assert_eq!(batch.events[1].text, "unfamiliar diagnostic");
        assert_eq!(log.position().offset, 0);
        log.retry();
        assert_eq!(
            log.read_batch(|| Ok(NOW), 32).expect("retry").events,
            batch.events
        );
        let position = log.acknowledge().expect("ack");
        assert_eq!(position.offset, input.len() as u64);
        let mut restarted = PgBouncerLog::new(path, position);
        assert!(
            restarted
                .read_batch(|| Ok(NOW), 32)
                .expect("restart")
                .events
                .is_empty()
        );
    }
}

#[test]
fn native_journal_body_cannot_supply_a_nested_header() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("journal.log");
    for body in [
        "tls_sbufio_recv: detail [12] DEBUG nested text",
        "tls_sbufio_recv: detail [12] WARNING nested text",
        "tls_sbufio_recv: app[12]: WARNING nested text",
    ] {
        std::fs::write(
            &path,
            format!("Mon 2026-09-21 08:17:37 EDT pgbouncer[1118]: {body}\n"),
        )
        .expect("input");
        let mut log = PgBouncerLog::new(path.clone(), Position::default());
        let events = log.read_batch(|| Ok(NOW), 8).expect("read").events;
        assert_eq!(events.len(), 1, "{body}");
        assert_eq!(events[0].text, body);
        assert_eq!(events[0].level, Level::Log);
        assert_eq!(events[0].pid, Some(1118));
        assert_eq!(events[0].ts, 1_789_993_057_000_000);
    }
}

#[test]
fn invalid_calendar_inner_header_keeps_its_metadata_and_batch_time() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("journal.log");
    for clock in [
        "bad-time",
        "2026-99-21 08:17:37.123 UTC",
        "2026-09-21 99:17:37",
    ] {
        std::fs::write(
            &path,
            format!(
                "Mon 2026-09-21 08:17:37 EDT pgbouncer[1118]: {clock} [66] ERROR invalid time\n"
            ),
        )
        .expect("input");
        let mut log = PgBouncerLog::new(path.clone(), Position::default());
        let events = log.read_batch(|| Ok(NOW), 8).expect("read").events;
        assert_eq!(events.len(), 1, "{clock}");
        assert_eq!(events[0].ts, NOW);
        assert_eq!(events[0].pid, Some(66));
        assert_eq!(events[0].level, Level::Error);
        assert_eq!(events[0].text, "invalid time");
    }
}
