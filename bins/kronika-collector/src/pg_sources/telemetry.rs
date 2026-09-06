//! Low-cardinality query traffic in the collector log.

use std::time::{Duration, Instant};

use kronika_source_pg::query::QueryStats;

use super::{ConnectionObservation, PgObservation, QueryObservation, QueryOutcome};
use crate::logging::{LogField, LogLevel, duration_ms, field, log_event, peak_rss_kib};

const REPORT_INTERVAL: Duration = Duration::from_mins(5);
const SLOW_QUERY: Duration = Duration::from_millis(500);
const MIB: u128 = 1_048_576;
const DECIMAL_PLACES: u128 = 1_000_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Totals {
    query_count: u64,
    rows: u64,
    application_payload_from_postgres_bytes: u64,
    application_payload_to_postgres_bytes: u64,
    batches: u64,
    query_errors: u64,
    sink_errors: u64,
    connect_errors: u64,
    query_timeouts: u64,
    connect_timeouts: u64,
    slow_queries: u64,
    fetch_elapsed: Duration,
    max_fetch_elapsed: Duration,
    encode_elapsed: Duration,
    append_elapsed: Duration,
    encoded_bytes: u64,
    wal_bytes_appended: u64,
}

impl Totals {
    fn record_query(&mut self, elapsed: Duration, stats: QueryStats, outcome: QueryOutcome) {
        let fetch_elapsed = stats.fetch_elapsed(elapsed);
        self.query_count = self.query_count.saturating_add(1);
        self.rows = self.rows.saturating_add(stats.rows);
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(stats.application_payload_from_postgres_bytes);
        self.application_payload_to_postgres_bytes = self
            .application_payload_to_postgres_bytes
            .saturating_add(stats.application_payload_to_postgres_bytes);
        self.batches = self.batches.saturating_add(stats.batches);
        self.query_errors = self
            .query_errors
            .saturating_add(u64::from(outcome == QueryOutcome::Error));
        self.sink_errors = self
            .sink_errors
            .saturating_add(u64::from(outcome == QueryOutcome::SinkError));
        self.query_timeouts = self
            .query_timeouts
            .saturating_add(u64::from(outcome == QueryOutcome::Timeout));
        self.slow_queries = self
            .slow_queries
            .saturating_add(u64::from(fetch_elapsed > SLOW_QUERY));
        self.fetch_elapsed = self.fetch_elapsed.saturating_add(fetch_elapsed);
        self.max_fetch_elapsed = self.max_fetch_elapsed.max(fetch_elapsed);
        self.encode_elapsed = self.encode_elapsed.saturating_add(stats.encode_elapsed);
        self.append_elapsed = self.append_elapsed.saturating_add(stats.append_elapsed);
        self.encoded_bytes = self.encoded_bytes.saturating_add(stats.encoded_bytes);
        self.wal_bytes_appended = self
            .wal_bytes_appended
            .saturating_add(stats.wal_bytes_appended);
    }

    const fn record_connection(&mut self, timeout: bool) {
        if timeout {
            self.connect_timeouts = self.connect_timeouts.saturating_add(1);
        } else {
            self.connect_errors = self.connect_errors.saturating_add(1);
        }
    }
}

/// Owns the interval aggregate and emits diagnostics for every observation.
#[derive(Debug)]
pub(crate) struct PgTelemetry {
    interval_started: Instant,
    totals: Totals,
    active: bool,
    shutdown_emitted: bool,
}

impl PgTelemetry {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            interval_started: now,
            totals: Totals::default(),
            active: false,
            shutdown_emitted: false,
        }
    }

    pub(crate) fn observe(&mut self, observation: PgObservation) {
        self.active = true;
        match observation {
            PgObservation::Query(observation) => {
                log_query(&observation);
                self.totals.record_query(
                    observation.elapsed,
                    observation.stats,
                    observation.outcome,
                );
            }
            PgObservation::Connection(observation) => {
                log_connection(&observation);
                self.totals.record_connection(observation.timeout);
            }
        }
    }

    /// Emit a five-minute aggregate after this telemetry has seen a source.
    pub(crate) fn maybe_emit(&mut self, now: Instant) {
        if self.active && now.saturating_duration_since(self.interval_started) >= REPORT_INTERVAL {
            self.report(now, "interval");
        }
    }

    /// Emit one final aggregate. Repeated shutdown paths do not duplicate it.
    pub(crate) fn shutdown(&mut self, now: Instant) {
        let Some(fields) = self.shutdown_fields(now, peak_rss_kib()) else {
            return;
        };
        log_event(LogLevel::Info, "pg_query_summary", &fields);
    }

    fn shutdown_fields(
        &mut self,
        now: Instant,
        peak_rss_kib: u64,
    ) -> Option<Vec<LogField<'static>>> {
        if self.shutdown_emitted {
            return None;
        }
        self.shutdown_emitted = true;
        Some(self.take_fields(now, "shutdown", peak_rss_kib))
    }

    fn report(&mut self, now: Instant, reason: &'static str) {
        let fields = self.take_fields(now, reason, peak_rss_kib());
        log_event(LogLevel::Info, "pg_query_summary", &fields);
    }

    fn take_fields(
        &mut self,
        now: Instant,
        reason: &'static str,
        peak_rss_kib: u64,
    ) -> Vec<LogField<'static>> {
        let interval = now.saturating_duration_since(self.interval_started);
        let totals = std::mem::take(&mut self.totals);
        self.interval_started = now;
        summary_fields(reason, interval, totals, peak_rss_kib)
    }
}

fn log_query(observation: &QueryObservation) {
    let fetch_elapsed = observation.stats.fetch_elapsed(observation.elapsed);
    let outcome = outcome_name(observation.outcome);
    let fields = query_fields(observation, fetch_elapsed, outcome);
    log_event(LogLevel::Debug, "pg_query_finish", &fields);
    if fetch_elapsed > SLOW_QUERY {
        log_event(LogLevel::Warn, "pg_query_slow", &fields);
    }
    if observation.outcome != QueryOutcome::Success {
        log_event(LogLevel::Warn, "pg_query_failure", &fields);
    }
}

fn query_fields<'a>(
    observation: &'a QueryObservation,
    fetch_elapsed: Duration,
    outcome: &'static str,
) -> Vec<LogField<'a>> {
    let mut fields = vec![
        field("query", observation.query_name),
        field("connection", observation.connection.as_str()),
        field("database", observation.database.as_str()),
        field("outcome", outcome),
        field("elapsed_ms", duration_ms(observation.elapsed)),
        field("fetch_elapsed_ms", duration_ms(fetch_elapsed)),
        field("rows", observation.stats.rows),
        field(
            "application_payload_from_postgres_bytes",
            observation.stats.application_payload_from_postgres_bytes,
        ),
        field(
            "application_payload_to_postgres_bytes",
            observation.stats.application_payload_to_postgres_bytes,
        ),
        field("batches", observation.stats.batches),
        field(
            "encode_elapsed_ms",
            duration_ms(observation.stats.encode_elapsed),
        ),
        field(
            "append_elapsed_ms",
            duration_ms(observation.stats.append_elapsed),
        ),
        field("encoded_bytes", observation.stats.encoded_bytes),
        field("wal_bytes_appended", observation.stats.wal_bytes_appended),
    ];
    if let Some(error) = observation.error.as_deref() {
        fields.push(field("error", error));
    }
    fields
}

fn log_connection(observation: &ConnectionObservation) {
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

fn summary_fields(
    reason: &'static str,
    interval: Duration,
    totals: Totals,
    peak_rss_kib: u64,
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
            totals.application_payload_from_postgres_bytes,
        ),
        field(
            "application_payload_from_postgres_mib",
            fixed_decimal(
                u128::from(totals.application_payload_from_postgres_bytes),
                MIB,
            ),
        ),
        field(
            "application_payload_to_postgres_bytes",
            totals.application_payload_to_postgres_bytes,
        ),
        field(
            "application_payload_to_postgres_mib",
            fixed_decimal(
                u128::from(totals.application_payload_to_postgres_bytes),
                MIB,
            ),
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

#[cfg(test)]
mod tests;
