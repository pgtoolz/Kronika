use super::*;

fn stats(rows: u64, received: u64, sent: u64) -> QueryStats {
    let mut stats = QueryStats::default();
    stats.rows = rows;
    stats.application_payload_from_postgres_bytes = received;
    stats.application_payload_to_postgres_bytes = sent;
    stats.batches = 2;
    stats.encode_elapsed = Duration::from_millis(7);
    stats.append_elapsed = Duration::from_millis(5);
    stats.encoded_bytes = 300;
    stats.wal_bytes_appended = 360;
    stats
}

#[test]
fn summary_has_stable_units_and_separate_failure_counts() {
    let mut totals = Totals::default();
    totals.record_query(
        Duration::from_millis(501),
        stats(4, 2 * 1_048_576, 1_048_576),
        QueryOutcome::Error,
    );
    totals.record_query(
        Duration::from_millis(500),
        stats(3, 1_048_576, 512 * 1024),
        QueryOutcome::Timeout,
    );
    totals.record_query(
        Duration::from_millis(20),
        stats(1, 0, 0),
        QueryOutcome::SinkError,
    );
    totals.record_connection(false);
    totals.record_connection(true);
    let fields = summary_fields("shutdown", Duration::from_secs(2), totals, 1234);
    let line = crate::logging::render_log_line(LogLevel::Info, "pg_query_summary", &fields);

    for expected in [
        "reason=shutdown",
        "query_count=3",
        "query_rate_per_s=1.500000",
        "rows=8",
        "application_payload_from_postgres_bytes=3145728",
        "application_payload_from_postgres_mib=3.000000",
        "application_payload_to_postgres_bytes=1572864",
        "application_payload_to_postgres_mib=1.500000",
        "batches=6",
        "query_errors=1",
        "sink_errors=1",
        "connect_errors=1",
        "query_timeouts=1",
        "connect_timeouts=1",
        "errors=3",
        "timeouts=2",
        "slow_queries=1",
        "fetch_elapsed_ms_total=1021",
        "fetch_elapsed_ms_max=501",
        "encoded_bytes=900",
        "wal_bytes_appended=1080",
        "peak_rss_kib=1234",
    ] {
        assert!(line.contains(expected), "{expected} is missing from {line}");
    }
}

#[test]
fn periodic_report_waits_for_five_minutes() {
    let started = Instant::now();
    let mut telemetry = PgTelemetry::new(started);
    telemetry.totals.record_query(
        Duration::from_millis(1),
        stats(1, 10, 5),
        QueryOutcome::Success,
    );
    telemetry.active = true;

    telemetry.maybe_emit(
        (started + REPORT_INTERVAL)
            .checked_sub(Duration::from_millis(1))
            .expect("one millisecond fits inside the report interval"),
    );
    assert_eq!(telemetry.totals.query_count, 1);
    telemetry.maybe_emit(started + REPORT_INTERVAL);
    assert_eq!(telemetry.totals.query_count, 0);
}

#[test]
fn query_line_has_safe_context_and_actionable_error() {
    let observation = QueryObservation {
        query_name: "pgbouncer_show_config",
        connection: "monitor@db.example:6432".to_owned(),
        database: "pgbouncer".to_owned(),
        elapsed: Duration::from_millis(12),
        stats: stats(2, 64, 11),
        outcome: QueryOutcome::Error,
        error: Some("permission denied for SHOW CONFIG".to_owned()),
    };
    let fields = query_fields(&observation, Duration::from_millis(12), "error");
    let line = crate::logging::render_log_line(LogLevel::Debug, "pg_query_finish", &fields);

    assert!(line.contains("connection=monitor@db.example:6432"));
    assert!(line.contains("database=pgbouncer"));
    assert!(line.contains("error=\"permission denied for SHOW CONFIG\""));
}

#[test]
fn shutdown_aggregate_is_emitted_once_even_when_empty() {
    let started = Instant::now();
    let mut telemetry = PgTelemetry::new(started);
    let first = telemetry.shutdown_fields(started + Duration::from_secs(1), 7);
    let second = telemetry.shutdown_fields(started + Duration::from_secs(2), 8);

    assert!(first.is_some());
    assert!(second.is_none());
}
