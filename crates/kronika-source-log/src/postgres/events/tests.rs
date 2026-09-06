use super::{
    AutovacuumKind, CheckpointPhase, Events, LifecycleKind, LockWaitKind, detail_list,
    parse_autovacuum, parse_checkpoint, parse_lifecycle, parse_lock_wait, parse_slow_query,
    parse_temp_file,
};
use crate::postgres::{ErrorCategory, PgRecord, Severity};

const TS: i64 = 1_780_000_000_000_000;

fn record(severity: Severity, message: &str) -> PgRecord {
    PgRecord::new(TS, severity, message)
}

/// Durations come out of the line exactly; this leaves room for the parse.
fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{actual} is not {expected}"
    );
}

fn collect(records: &[PgRecord]) -> Events {
    let mut events = Events::default();
    for record in records {
        events.add(record);
    }
    events.finish();
    events
}

#[test]
fn a_finished_checkpoint_carries_its_phases_and_its_wal_files() {
    let event = parse_checkpoint(
        "checkpoint complete: wrote 3 buffers (0.0%); 1 WAL file(s) added, 2 removed, \
         3 recycled; write=0.201 s, sync=0.011 s, total=0.230 s; sync files=4, \
         longest=0.008 s, average=0.006 s; distance=512 kB, estimate=1024 kB",
        TS,
    )
    .expect("a checkpoint");

    assert_eq!(event.phase, CheckpointPhase::Complete);
    assert_eq!(event.buffers_written, Some(3));
    assert_eq!(
        (event.wal_added, event.wal_removed, event.wal_recycled),
        (Some(1), Some(2), Some(3))
    );
    assert_eq!(event.write_ms, Some(201.0));
    assert_eq!(event.total_ms, Some(230.0));
    assert_eq!(event.sync_files, Some(4));
    assert_eq!(event.distance_kb, Some(512));
    assert_eq!(event.estimate_kb, Some(1024));
}

#[test]
fn a_starting_checkpoint_carries_its_reason() {
    let event = parse_checkpoint("checkpoint starting: wal", TS).expect("a checkpoint");

    assert_eq!(event.phase, CheckpointPhase::Starting);
    assert_eq!(event.reason, Some("wal".to_owned()));
    assert_eq!(event.buffers_written, None);
}

#[test]
fn a_too_frequent_checkpoint_carries_the_interval_it_names() {
    let event = parse_checkpoint(
        "checkpoints are occurring too frequently (9 seconds apart)",
        TS,
    )
    .expect("a checkpoint");

    assert_eq!(event.phase, CheckpointPhase::TooFrequent);
    assert_eq!(event.seconds_apart, Some(9));
}

#[test]
fn an_autovacuum_report_carries_every_group_it_prints() {
    let event = parse_autovacuum(
        "automatic vacuum of table \"shop.public.orders\": index scans: 1 \
         pages: 0 removed, 45 remain, 0 skipped due to pins, 0 skipped frozen \
         tuples: 100 removed, 200 remain, 7 are dead but not yet removable, oldest xmin: 800 \
         buffer usage: 90 hits, 3 misses, 5 dirtied \
         avg read rate: 1.250 MB/s, avg write rate: 2.500 MB/s \
         system usage: CPU: user: 0.010 s, system: 0.020 s, elapsed: 0.050 s \
         WAL usage: 12 records, 3 full page images, 4567 bytes",
        TS,
    )
    .expect("an autovacuum report");

    assert_eq!(event.kind, AutovacuumKind::Vacuum);
    assert_eq!(event.relation, Some("shop.public.orders".to_owned()));
    assert_eq!(event.index_scans, Some(1));
    assert_eq!(
        (event.pages_removed, event.pages_remaining),
        (Some(0), Some(45))
    );
    assert_eq!(
        (event.tuples_removed, event.tuples_remaining),
        (Some(100), Some(200))
    );
    assert_eq!(event.tuples_dead_not_removable, Some(7));
    assert_eq!(
        (event.buffer_hits, event.buffer_misses, event.buffer_dirtied),
        (Some(90), Some(3), Some(5))
    );
    assert_eq!(event.avg_read_rate_mbs, Some(1.25));
    assert_eq!(event.avg_write_rate_mbs, Some(2.5));
    assert_eq!(
        (event.cpu_user_ms, event.cpu_system_ms),
        (Some(10.0), Some(20.0))
    );
    assert_eq!(event.elapsed_ms, Some(50.0));
    assert_eq!(
        (event.wal_records, event.wal_fpi, event.wal_bytes),
        (Some(12), Some(3), Some(4567))
    );
}

#[test]
fn an_analyze_report_leaves_the_vacuum_only_numbers_missing() {
    let event = parse_autovacuum(
        "automatic analyze of table \"shop.public.orders\" \
         buffer usage: 12 hits, 1 misses, 0 dirtied \
         system usage: CPU: user: 0.000 s, system: 0.000 s, elapsed: 0.010 s",
        TS,
    )
    .expect("an analyze report");

    assert_eq!(event.kind, AutovacuumKind::Analyze);
    assert_eq!(event.tuples_removed, None);
    assert_eq!(event.tuples_remaining, None);
    assert_eq!(event.buffer_hits, Some(12));
}

#[test]
fn a_slow_statement_carries_its_duration_and_its_sql() {
    let (duration_ms, sql) =
        parse_slow_query("duration: 1234.567 ms  statement: select * from orders where id = 1")
            .expect("a slow statement");

    close(duration_ms, 1234.567);
    assert_eq!(sql, "select * from orders where id = 1");
}

#[test]
fn a_message_that_only_mentions_a_duration_is_not_a_slow_statement() {
    assert_eq!(parse_slow_query("duration: 12.0 ms"), None);
    assert_eq!(parse_slow_query("connection authorized: user=alice"), None);
}

#[test]
fn a_lock_wait_carries_the_waiter_the_mode_and_the_target() {
    let event = parse_lock_wait(
        "process 12345 still waiting for ShareLock on transaction 987 after 1000.123 ms",
        TS,
    )
    .expect("a lock wait");

    assert_eq!(event.kind, LockWaitKind::Waiting);
    assert_eq!(event.pid, Some(12_345));
    assert_eq!(event.lock_mode, Some("ShareLock".to_owned()));
    assert_eq!(event.lock_target, Some("transaction 987".to_owned()));
    assert_eq!(event.duration_ms, Some(1000.123));
}

#[test]
fn a_lock_wait_detail_yields_the_holders_and_the_queue() {
    assert_eq!(
        detail_list(
            "Process holding the lock: 583. Wait queue: 2078, 456.",
            "holding the lock: "
        ),
        Some("583")
    );
    assert_eq!(
        detail_list(
            "Processes holding the lock: 101, 102. Wait queue: 2078.",
            "holding the lock: "
        ),
        Some("101, 102")
    );
    assert_eq!(
        detail_list(
            "Process holding the lock: 583. Wait queue: 2078, 456.",
            "Wait queue: "
        ),
        Some("2078, 456")
    );
    assert_eq!(
        detail_list("Key (id)=(1) already exists.", "Wait queue: "),
        None
    );
}

#[test]
fn an_acquired_lock_is_its_own_kind() {
    let event = parse_lock_wait(
        "process 12345 acquired ShareLock on transaction 987 after 2000.000 ms",
        TS,
    )
    .expect("a lock wait");

    assert_eq!(event.kind, LockWaitKind::Acquired);
}

#[test]
fn a_temporary_file_carries_its_path_and_its_size() {
    let event = parse_temp_file(
        "temporary file: path \"base/pgsql_tmp/pgsql_tmp1234.0\", size 1048576",
        TS,
    )
    .expect("a temporary file");

    assert_eq!(
        event.path,
        Some("base/pgsql_tmp/pgsql_tmp1234.0".to_owned())
    );
    assert_eq!(event.size_bytes, 1_048_576);
}

#[test]
fn lifecycle_records_are_told_apart() {
    let crash = parse_lifecycle(
        "server process (PID 4242) was terminated by signal 9: Killed",
        TS,
    )
    .expect("a crash");
    assert_eq!(crash.kind, LifecycleKind::Crash);
    assert_eq!(crash.pid, Some(4242));
    assert_eq!(crash.signal, Some(9));

    let shutdown = parse_lifecycle("received fast shutdown request", TS).expect("a shutdown");
    assert_eq!(shutdown.kind, LifecycleKind::Shutdown);
    assert_eq!(shutdown.shutdown_mode, Some("fast".to_owned()));

    let ready = parse_lifecycle("database system is ready to accept connections", TS)
        .expect("a ready record");
    assert_eq!(ready.kind, LifecycleKind::Ready);

    assert_eq!(
        parse_lifecycle("connection received: host=[local]", TS),
        None
    );
}

#[test]
fn errors_that_differ_only_in_their_values_share_one_row() {
    let events = collect(&[
        record(Severity::Error, "relation \"a\" does not exist"),
        record(Severity::Error, "relation \"b\" does not exist"),
        record(Severity::Error, "division by zero"),
    ]);

    assert_eq!(events.errors.len(), 2);
    assert_eq!(events.errors[0].count, 2);
    assert_eq!(events.errors[0].category, ErrorCategory::Syntax);
    assert_eq!(events.errors[0].sample, "relation \"a\" does not exist");
    assert_eq!(events.errors[1].count, 1);
}

#[test]
fn statements_that_differ_only_in_their_literals_share_one_row() {
    let events = collect(&[
        record(
            Severity::Log,
            "duration: 100.0 ms  statement: select * from orders where id = 1",
        ),
        record(
            Severity::Log,
            "duration: 300.0 ms  statement: select * from orders where id = 2",
        ),
    ]);

    assert_eq!(events.slow_queries.len(), 1);
    let query = &events.slow_queries[0];
    assert_eq!(query.count, 2);
    close(query.max_duration_ms, 300.0);
    close(query.total_duration_ms, 400.0);
    assert_eq!(query.sample, "select * from orders where id = 2");
}

#[test]
fn a_log_record_that_matches_no_shape_produces_nothing() {
    let events = collect(&[record(Severity::Log, "connection authorized: user=alice")]);

    assert!(events.is_empty());
}

#[test]
fn an_aggressive_autovacuum_report_is_still_a_vacuum() {
    let event = parse_autovacuum(
        "automatic aggressive vacuum of table \"shop.public.orders\": index scans: 1",
        TS,
    )
    .expect("an aggressive vacuum report");
    assert_eq!(event.kind, AutovacuumKind::Vacuum);
    assert_eq!(event.relation, Some("shop.public.orders".to_owned()));

    let wraparound = parse_autovacuum(
        "automatic vacuum to prevent wraparound of table \"shop.public.orders\": index scans: 0",
        TS,
    )
    .expect("a wraparound vacuum report");
    assert_eq!(wraparound.kind, AutovacuumKind::Vacuum);
}

#[test]
fn an_extended_protocol_execute_is_a_slow_statement() {
    let (duration_ms, sql) =
        parse_slow_query("duration: 250.500 ms  execute stmt_7: select * from orders")
            .expect("an execute line");
    close(duration_ms, 250.5);
    assert_eq!(sql, "select * from orders");

    let (_, unnamed) = parse_slow_query("duration: 9.100 ms  execute <unnamed>: select 1")
        .expect("an unnamed execute line");
    assert_eq!(unnamed, "select 1");

    assert_eq!(
        parse_slow_query("duration: 1.000 ms  bind stmt_7: select 1"),
        None
    );
    assert_eq!(
        parse_slow_query("duration: 1.000 ms  parse stmt_7: select 1"),
        None
    );
}
