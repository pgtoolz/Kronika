//! What the parsers make of a real log file.
//!
//! The fixtures under `tests/fixtures` are log files as `PostgreSQL` and
//! `PgBouncer` write them. Each ends with a line the collector does not record,
//! which closes the record before it: a record still open when a read reaches
//! the end of the file waits for whatever might continue it.

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

#[test]
fn a_journalctl_prefix_in_front_of_pgbouncer_lines_is_skipped() {
    let mut plain = PgBouncerLog::new(fixture("pgbouncer.log"), Position::default());
    let plain = plain
        .read_batch(1024)
        .expect("read the plain fixture")
        .events;

    let mut log = PgBouncerLog::new(fixture("pgbouncer-journald.log"), Position::default());
    let batch = log.read_batch(1024).expect("read the prefixed fixture");
    if batch.needs_ack {
        log.acknowledge().expect("acknowledge the fixture");
    }
    let events = batch.events;

    assert_eq!(
        &events[..plain.len()],
        &plain[..],
        "short, short-full and short-iso prefixes, with or without a host name, \
         leave the pooler's own time, level, socket and text untouched"
    );
    let texts: Vec<&str> = events
        .iter()
        .skip(plain.len())
        .map(|event| event.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "query_timeout",
            "process up: PgBouncer 1.16.0, libevent 2.1.11-stable (epoll), adns: c-ares 1.15.0, tls: OpenSSL 1.1.1f  31 Mar 2020",
        ],
        "a prefixed line is still dropped when its message is not a recognized event"
    );
}

const POOLER_ERROR_TEXTS: [&str; 7] = [
    "query_wait_timeout",
    "no such user",
    "server login failed: FATAL database \"nope\" does not exist",
    "query_wait_timeout",
    "query_wait_timeout",
    "password authentication failed",
    "\"trust\" authentication failed",
];

#[test]
fn a_pooler_error_is_an_event_unless_its_closing_line_came_first() {
    let mut log = PgBouncerLog::new(fixture("pgbouncer-pooler-errors.log"), Position::default());

    let batch = log.read_batch(1024).expect("read the fixture");
    if batch.needs_ack {
        log.acknowledge().expect("acknowledge the fixture");
    }
    let events = batch.events;

    let texts: Vec<&str> = events.iter().map(|event| event.text.as_str()).collect();
    assert_eq!(
        texts, POOLER_ERROR_TEXTS,
        "the twin of a closing line is dropped, a lone pooler error stays, \
         an unrecognized pooler error and a relayed server message are not events"
    );

    assert_eq!(
        events[0].level,
        Level::Log,
        "the closing line is kept, not its twin"
    );
    assert_eq!(events[0].username.as_deref(), Some("alice"));
    assert_eq!(events[1].level, Level::Warning);
    assert_eq!(events[1].username.as_deref(), Some("bob"));
    assert_eq!(events[1].host.as_deref(), Some("10.0.0.3"));
    assert_eq!(events[3].username.as_deref(), Some("dave"));
    assert_eq!(
        events[4].username.as_deref(),
        Some("erin"),
        "the same reason on another socket is not a twin"
    );
    assert_eq!(
        events[6].username.as_deref(),
        Some("grace"),
        "an hba method failure names the method, so it is matched by its shape"
    );
}

#[test]
fn pooler_error_deduplication_survives_a_batch_boundary() {
    let mut log = PgBouncerLog::new(fixture("pgbouncer-pooler-errors.log"), Position::default());
    let mut texts = Vec::new();
    loop {
        let batch = log.read_batch(1).expect("read one record");
        texts.extend(batch.events.into_iter().map(|event| event.text));
        if batch.needs_ack {
            log.acknowledge().expect("acknowledge one record");
        }
        if batch.at_eof {
            break;
        }
    }
    assert_eq!(texts, POOLER_ERROR_TEXTS);
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
        assert_eq!(position.offset, complete.len() as u64);
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
fn bare_warning_waits_for_a_complete_line_then_accepts_details_on_the_next_tick() {
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
    file.write_all(b"\n").expect("finish line");
    assert!(
        log.read_batch(|| Ok(NOW), 1, 900)
            .expect("newline read")
            .events
            .is_empty()
    );
    file.write_all(b"DETAIL:  detail body\nHINT:  hint body\n2026-09-14 10:13:00 GMT [1] => [1-1] client=,db=,user= LOG:  database system is ready to accept connections\n").expect("next tick");
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
    log.read_batch(|| Ok(NOW), 1, 900).expect("stage next line");
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
    assert_eq!(
        log.position().offset,
        (input.len() - "INFO:  sentinel\n".len()) as u64
    );
}
