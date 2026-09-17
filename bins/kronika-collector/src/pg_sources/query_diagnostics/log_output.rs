//! Stable event names, field names, and units used in the collector log.

use std::time::Duration;

use super::{ConnectionObservation, QueryObservation, QueryOutcome, SLOW_QUERY, Totals};
use crate::logging::{LogField, LogLevel, duration_ms, field, log_event};

const MIB: u128 = 1_048_576;
const DECIMAL_PLACES: u128 = 1_000_000;

pub(super) fn log_query(observation: &QueryObservation) {
    let fetch_elapsed = observation.stats.fetch_elapsed(observation.elapsed);
    let fields = query_fields(observation);
    log_event(LogLevel::Debug, "pg_query_finish", &fields);
    if fetch_elapsed > SLOW_QUERY {
        log_event(LogLevel::Warn, "pg_query_slow", &fields);
    }
    if observation.outcome != QueryOutcome::Success {
        log_event(LogLevel::Warn, "pg_query_failure", &fields);
    }
}

pub(super) fn query_fields(observation: &QueryObservation) -> Vec<LogField<'_>> {
    let stats = observation.stats;
    let fetch_elapsed = stats.fetch_elapsed(observation.elapsed);
    let mut fields = vec![
        field("query", observation.query_name),
        field("connection", observation.connection.as_str()),
        field("database", observation.database.as_str()),
        field("outcome", outcome_name(observation.outcome)),
        field("elapsed_ms", duration_ms(observation.elapsed)),
        field("fetch_elapsed_ms", duration_ms(fetch_elapsed)),
        field("rows", stats.rows),
        field(
            "application_payload_from_postgres_bytes",
            stats.application_payload_from_postgres_bytes,
        ),
        field(
            "application_payload_to_postgres_bytes",
            stats.application_payload_to_postgres_bytes,
        ),
        field("batches", stats.batches),
        field("encode_elapsed_ms", duration_ms(stats.encode_elapsed)),
        field("append_elapsed_ms", duration_ms(stats.append_elapsed)),
        field("encoded_bytes", stats.encoded_bytes),
        field("wal_bytes_appended", stats.wal_bytes_appended),
    ];
    if let Some(error) = observation.error.as_deref() {
        fields.push(field("error", error));
    }
    fields
}

pub(super) fn log_connection(observation: &ConnectionObservation) {
    log_event(
        LogLevel::Warn,
        "pg_connection_failure",
        &[
            field("connection", observation.connection.as_str()),
            field("database", observation.database.as_str()),
            field("elapsed_ms", duration_ms(observation.elapsed)),
            field("timeout", observation.timeout),
            field("closed", observation.closed),
            field("error", observation.error.as_str()),
        ],
    );
}

const fn outcome_name(outcome: QueryOutcome) -> &'static str {
    match outcome {
        QueryOutcome::Success => "success",
        QueryOutcome::Error => "error",
        QueryOutcome::Timeout => "timeout",
        QueryOutcome::SinkError => "sink_error",
    }
}

fn rate_per_second(count: u64, interval: Duration) -> String {
    let nanos = interval.as_nanos();
    if nanos == 0 {
        return "0.000000".to_owned();
    }
    fixed_decimal(u128::from(count).saturating_mul(1_000_000_000), nanos)
}

fn fixed_decimal(numerator: u128, denominator: u128) -> String {
    let scaled = numerator
        .saturating_mul(DECIMAL_PLACES)
        .saturating_add(denominator / 2)
        / denominator;
    let whole = scaled / DECIMAL_PLACES;
    let fraction = scaled % DECIMAL_PLACES;
    format!("{whole}.{fraction:06}")
}

pub(super) fn summary_fields(
    reason: &'static str,
    interval: Duration,
    totals: Totals,
    peak_rss_kib: Option<u64>,
) -> Vec<LogField<'static>> {
    let timeouts = totals
        .query_timeouts
        .saturating_add(totals.connect_timeouts);
    let errors = totals
        .query_errors
        .saturating_add(totals.sink_errors)
        .saturating_add(totals.connect_errors);
    vec![
        field("reason", reason),
        field("payload_measure", "logical_application_estimate"),
        field("interval_ms", duration_ms(interval)),
        field("query_count", totals.query_count),
        field(
            "query_rate_per_s",
            rate_per_second(totals.query_count, interval),
        ),
        field("rows", totals.rows),
        field(
            "application_payload_from_postgres_bytes",
            totals.received_bytes,
        ),
        field(
            "application_payload_from_postgres_mib",
            fixed_decimal(u128::from(totals.received_bytes), MIB),
        ),
        field("application_payload_to_postgres_bytes", totals.sent_bytes),
        field(
            "application_payload_to_postgres_mib",
            fixed_decimal(u128::from(totals.sent_bytes), MIB),
        ),
        field("batches", totals.batches),
        field("query_errors", totals.query_errors),
        field("sink_errors", totals.sink_errors),
        field("connect_errors", totals.connect_errors),
        field("query_timeouts", totals.query_timeouts),
        field("connect_timeouts", totals.connect_timeouts),
        field("errors", errors),
        field("timeouts", timeouts),
        field("slow_queries", totals.slow_queries),
        field("fetch_elapsed_ms_total", duration_ms(totals.fetch_elapsed)),
        field(
            "fetch_elapsed_ms_max",
            duration_ms(totals.max_fetch_elapsed),
        ),
        field(
            "encode_elapsed_ms_total",
            duration_ms(totals.encode_elapsed),
        ),
        field(
            "append_elapsed_ms_total",
            duration_ms(totals.append_elapsed),
        ),
        field("encoded_bytes", totals.encoded_bytes),
        field("wal_bytes_appended", totals.wal_bytes_appended),
        field("peak_rss_kib", peak_rss_kib),
    ]
}
