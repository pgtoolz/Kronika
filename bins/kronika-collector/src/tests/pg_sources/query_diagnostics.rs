use super::log_output::query_fields;
use super::*;

#[test]
fn source_warnings_do_not_change_query_totals_or_start_periodic_summaries() {
    let mut diagnostics = PgQueryDiagnostics::new(Instant::now());
    for warning in [
        kronika_source_pg::PgWarning::StatsVisibilityRequired {
            database: "metrics".to_owned(),
        },
        kronika_source_pg::PgWarning::StatementsExtensionUpdateRequired {
            database: "metrics".to_owned(),
            extension_version: "1.8".to_owned(),
        },
    ] {
        diagnostics.observe(PgObservation::Warning(warning));
    }
    assert!(!diagnostics.has_observations);
    assert_eq!(diagnostics.totals, Totals::default());
}
use kronika_source_pg::query::QueryStats;

fn query(elapsed: Duration, stats: QueryStats, outcome: QueryOutcome) -> QueryObservation {
    QueryObservation {
        query_name: "test_query",
        connection: "monitor@db.example:5432".to_owned(),
        database: "postgres".to_owned(),
        elapsed,
        stats,
        outcome,
        error: None,
    }
}

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
    totals.record_query(&query(
        Duration::from_millis(501),
        stats(4, 2 * 1_048_576, 1_048_576),
        QueryOutcome::Error,
    ));
    totals.record_query(&query(
        Duration::from_millis(500),
        stats(3, 1_048_576, 512 * 1024),
        QueryOutcome::Timeout,
    ));
    totals.record_query(&query(
        Duration::from_millis(20),
        stats(1, 0, 0),
        QueryOutcome::SinkError,
    ));
    totals.record_connection(false);
    totals.record_connection(true);
    let fields = summary_fields("shutdown", Duration::from_secs(2), totals, Some(1234));
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
    let mut diagnostics = PgQueryDiagnostics::new(started);
    diagnostics.observe(PgObservation::Query(query(
        Duration::from_millis(1),
        stats(1, 10, 5),
        QueryOutcome::Success,
    )));

    diagnostics.maybe_emit(
        (started + REPORT_INTERVAL)
            .checked_sub(Duration::from_millis(1))
            .expect("one millisecond fits inside the report interval"),
    );
    assert_eq!(diagnostics.totals.query_count, 1);
    diagnostics.maybe_emit(started + REPORT_INTERVAL);
    assert_eq!(diagnostics.totals.query_count, 0);
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
    let fields = query_fields(&observation);
    let line = crate::logging::render_log_line(LogLevel::Debug, "pg_query_finish", &fields);

    assert!(line.contains("connection=monitor@db.example:6432"));
    assert!(line.contains("database=pgbouncer"));
    assert!(line.contains("error=\"permission denied for SHOW CONFIG\""));
}

#[test]
fn shutdown_aggregate_is_emitted_once_even_when_empty() {
    let started = Instant::now();
    let mut diagnostics = PgQueryDiagnostics::new(started);
    let shutdown_at = started + Duration::from_secs(1);
    diagnostics.shutdown(shutdown_at);
    assert!(diagnostics.shutdown_emitted);
    assert_eq!(diagnostics.interval_started, shutdown_at);

    diagnostics.shutdown(started + Duration::from_secs(2));
    assert_eq!(diagnostics.interval_started, shutdown_at);
}

#[test]
fn slow_query_boundary_uses_fetch_time_without_the_sink() {
    let mut stats = QueryStats::default();
    stats.record_batch_write(
        Duration::from_millis(600),
        kronika_source_pg::query::BatchWrite {
            encode_elapsed: Duration::from_millis(200),
            append_elapsed: Duration::from_millis(300),
            ..kronika_source_pg::query::BatchWrite::default()
        },
    );
    for (elapsed, expected_fetch, expected_slow) in [
        (Duration::from_millis(1100), Duration::from_millis(500), 0),
        (
            Duration::from_millis(1100) + Duration::from_nanos(1),
            Duration::from_millis(500) + Duration::from_nanos(1),
            1,
        ),
        (Duration::from_millis(400), Duration::ZERO, 0),
    ] {
        let mut totals = Totals::default();
        totals.record_query(&query(elapsed, stats, QueryOutcome::Success));

        assert_eq!(totals.fetch_elapsed, expected_fetch);
        assert_eq!(totals.max_fetch_elapsed, expected_fetch);
        assert_eq!(totals.slow_queries, expected_slow);
    }
}

#[test]
fn connection_failures_activate_reports_and_each_report_restarts_the_interval() {
    let started = Instant::now();
    let mut diagnostics = PgQueryDiagnostics::new(started);
    let first_deadline = started + REPORT_INTERVAL;
    diagnostics.maybe_emit(first_deadline);
    assert_eq!(diagnostics.interval_started, started);
    let failure = |timeout| {
        PgObservation::Connection(ConnectionObservation {
            connection: "monitor@db.example:5432".to_owned(),
            database: "postgres".to_owned(),
            elapsed: Duration::from_millis(1),
            timeout,
            closed: true,
            error: "connection unavailable".to_owned(),
        })
    };

    diagnostics.observe(failure(false));
    assert_eq!(diagnostics.totals.connect_errors, 1);
    diagnostics.maybe_emit(first_deadline);
    assert_eq!(diagnostics.totals, Totals::default());
    assert_eq!(diagnostics.interval_started, first_deadline);

    diagnostics.observe(failure(true));
    let second_deadline = first_deadline + REPORT_INTERVAL;
    diagnostics.maybe_emit(
        second_deadline
            .checked_sub(Duration::from_nanos(1))
            .expect("one nanosecond fits inside the report interval"),
    );
    assert_eq!(diagnostics.totals.connect_timeouts, 1);
    diagnostics.maybe_emit(second_deadline);
    assert_eq!(diagnostics.totals, Totals::default());
    assert_eq!(diagnostics.interval_started, second_deadline);

    let idle_deadline = second_deadline + REPORT_INTERVAL;
    diagnostics.maybe_emit(idle_deadline);
    assert_eq!(diagnostics.totals, Totals::default());
    assert_eq!(diagnostics.interval_started, idle_deadline);
}

#[test]
fn query_rates_have_six_decimal_places_and_round_fractional_values() {
    for (count, interval, expected) in [
        (7, Duration::ZERO, "0.000000"),
        (0, Duration::from_secs(3), "0.000000"),
        (1, Duration::from_secs(2), "0.500000"),
        (1, Duration::from_secs(3), "0.333333"),
        (2, Duration::from_secs(3), "0.666667"),
        (1, Duration::from_secs(2_000_000), "0.000001"),
    ] {
        let fields = summary_fields(
            "interval",
            interval,
            Totals {
                query_count: count,
                ..Totals::default()
            },
            None,
        );
        let line = crate::logging::render_log_line(LogLevel::Info, "pg_query_summary", &fields);
        assert!(
            line.split_whitespace()
                .any(|field| field == format!("query_rate_per_s={expected}"))
        );
    }
}

#[test]
fn accumulated_counts_durations_and_failure_summaries_saturate() {
    let mut totals = Totals {
        query_count: u64::MAX,
        rows: u64::MAX - 1,
        received_bytes: u64::MAX - 1,
        query_errors: u64::MAX,
        connect_errors: u64::MAX,
        query_timeouts: u64::MAX,
        connect_timeouts: u64::MAX,
        fetch_elapsed: Duration::MAX,
        ..Totals::default()
    };
    totals.record_query(&query(
        Duration::from_millis(1),
        stats(2, 2, 0),
        QueryOutcome::Error,
    ));
    totals.record_connection(false);
    totals.record_connection(true);

    assert_eq!(totals.query_count, u64::MAX);
    assert_eq!(totals.rows, u64::MAX);
    assert_eq!(totals.received_bytes, u64::MAX);
    assert_eq!(totals.fetch_elapsed, Duration::MAX);
    let fields = summary_fields("interval", Duration::from_secs(1), totals, None);
    let line = crate::logging::render_log_line(LogLevel::Info, "pg_query_summary", &fields);
    for expected in [
        format!("errors={}", u64::MAX),
        format!("timeouts={}", u64::MAX),
    ] {
        assert!(line.split_whitespace().any(|field| field == expected));
    }
}
