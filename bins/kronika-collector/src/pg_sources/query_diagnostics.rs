//! Log collector SQL timings, failures, and five-minute summaries.

mod log_output;

use std::time::{Duration, Instant};

use super::{ConnectionObservation, PgObservation, QueryObservation, QueryOutcome};
use crate::logging::{LogLevel, log_event, peak_rss_kib};
use log_output::{log_connection, log_query, summary_fields};

/// Minimum interval between periodic `pg_query_summary` log entries.
/// Periodic reports start after the first query or connection failure;
/// shutdown emits its final summary without waiting for this interval.
const REPORT_INTERVAL: Duration = Duration::from_mins(5);

/// Fetch-time threshold for `pg_query_slow` warnings and the `slow_queries` counter.
/// A query is slow only when its fetch time is strictly greater than this value.
/// Fetch time excludes synchronous batch processing, including encoding and WAL writes.
const SLOW_QUERY: Duration = Duration::from_millis(500);

/// Log each query or connection failure and accumulate interval totals.
#[derive(Debug)]
pub(crate) struct PgQueryDiagnostics {
    interval_started: Instant,
    totals: Totals,
    has_observations: bool,
    shutdown_emitted: bool,
}

impl PgQueryDiagnostics {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            interval_started: now,
            totals: Totals::default(),
            has_observations: false,
            shutdown_emitted: false,
        }
    }

    pub(crate) fn observe(&mut self, observation: PgObservation) {
        self.has_observations = true;
        match observation {
            PgObservation::Query(observation) => {
                log_query(&observation);
                self.totals.record_query(&observation);
            }
            PgObservation::Connection(observation) => {
                log_connection(&observation);
                self.totals.record_connection(observation.timeout);
            }
        }
    }

    /// Start periodic reports after the first observation, including later idle intervals.
    pub(crate) fn maybe_emit(&mut self, now: Instant) {
        if self.has_observations
            && now.saturating_duration_since(self.interval_started) >= REPORT_INTERVAL
        {
            self.report(now, "interval");
        }
    }

    /// Emit the remaining totals once, even if nothing was collected.
    pub(crate) fn shutdown(&mut self, now: Instant) {
        if self.shutdown_emitted {
            return;
        }
        self.shutdown_emitted = true;
        self.report(now, "shutdown");
    }

    fn report(&mut self, now: Instant, reason: &'static str) {
        let interval = now.saturating_duration_since(self.interval_started);
        let totals = std::mem::take(&mut self.totals);
        self.interval_started = now;
        let fields = summary_fields(reason, interval, totals, peak_rss_kib());
        log_event(LogLevel::Info, "pg_query_summary", &fields);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Totals {
    query_count: u64,
    rows: u64,
    // Logical application payload; these are not wire or TLS byte counts.
    received_bytes: u64,
    sent_bytes: u64,
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
    fn record_query(&mut self, observation: &QueryObservation) {
        let stats = observation.stats;
        let fetch_elapsed = stats.fetch_elapsed(observation.elapsed);
        self.query_count = self.query_count.saturating_add(1);
        self.rows = self.rows.saturating_add(stats.rows);
        self.received_bytes = self
            .received_bytes
            .saturating_add(stats.application_payload_from_postgres_bytes);
        self.sent_bytes = self
            .sent_bytes
            .saturating_add(stats.application_payload_to_postgres_bytes);
        self.batches = self.batches.saturating_add(stats.batches);
        match observation.outcome {
            QueryOutcome::Success => {}
            QueryOutcome::Error => self.query_errors = self.query_errors.saturating_add(1),
            QueryOutcome::SinkError => self.sink_errors = self.sink_errors.saturating_add(1),
            QueryOutcome::Timeout => self.query_timeouts = self.query_timeouts.saturating_add(1),
        }
        if fetch_elapsed > SLOW_QUERY {
            self.slow_queries = self.slow_queries.saturating_add(1);
        }
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

#[cfg(test)]
#[path = "../tests/pg_sources/query_diagnostics.rs"]
mod tests;
