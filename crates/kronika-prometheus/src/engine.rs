//! Exporter pass driver: per-database scheduling, derived availability,
//! metric execution.
//!
//! One pass runs on the collector's tick: databases sequentially, one metric
//! at a time (EXE-6). Intervals compare against the previous run's start,
//! successful or not (EXE-7). `42P01`/`42883` errors disable a metric for a
//! database until the next discovery refresh (EXE-8); databases gone from
//! discovery are dropped with their state and connection (EXE-10).
//! `instance_up` is derived from connection health: a working connection or
//! successful setup probe is 1, a connection-level failure (connect error,
//! connection lost, deadline expiry) is 0, and SQL-level errors never zero
//! it. A connection-level failure during a query drops the connection,
//! reconnects with the full setup, and retries the same query once.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cache::{DbCache, Entry, INSTANCE_UP_INTERVAL_S};
use crate::catalog::{MetricDef, NOT_EXPOSED_METRICS, NodeStatus};
use crate::executor::{ExecutorFactory, INSTANCE_UP_METRIC, MetricError, ServerFacts, SqlExecutor};
use crate::measurement::{SampleSet, instance_up_sample_set, to_sample_set};
use crate::schedule::due;

/// EXE-9 reporting hook: (database, metric, error text or "cleared").
type ErrorCallback = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// One resolved preset metric.
#[derive(Debug, Clone)]
pub struct PresetMetric {
    /// Catalog metric name.
    pub name: String,
    /// Definition clone for the pass.
    pub def: MetricDef,
    /// Collection interval seconds.
    pub interval_s: u64,
}

/// Per-database exporter state: cache, connection verdict, executor.
struct DbState<F: ExecutorFactory> {
    cache: DbCache,
    /// Whether the exporter connection currently works (derived `instance_up`).
    connected: bool,
    /// Last `instance_up` value stored, with its stamp.
    stored_up: Option<(bool, i64)>,
    /// Facts from the connection setup probe.
    facts: Option<ServerFacts>,
    /// Start of the last run of each metric, epoch ms (EXE-7).
    last_metric_start_ms: BTreeMap<String, i64>,
    /// Metrics disabled until the next discovery refresh (EXE-8).
    disabled: Vec<String>,
    /// The database's executor, owning its connection. `None` after a
    /// connection-level failure; the next pass reopens it.
    sql: Option<F::Sql>,
}

impl<F: ExecutorFactory> DbState<F> {
    fn new() -> Self {
        Self {
            cache: DbCache::default(),
            connected: false,
            stored_up: None,
            facts: None,
            last_metric_start_ms: BTreeMap::new(),
            disabled: Vec::new(),
            sql: None,
        }
    }
}

/// Exporter state: scheduling, caches and self metrics.
#[expect(
    missing_debug_implementations,
    reason = "generic over the executor factory; debug printing adds nothing"
)]
pub struct Exporter<F: ExecutorFactory> {
    factory: F,
    preset: Vec<PresetMetric>,
    /// Start of the last run of each instance-level metric, epoch ms.
    instance_last_start_ms: BTreeMap<String, i64>,
    /// Last instance-level fetch per storage-resolved metric with the
    /// database that produced it, relabeled per database on publish.
    instance_sets: BTreeMap<String, (String, SampleSet)>,
    per_db: BTreeMap<String, DbState<F>>,
    fetch_errors: BTreeMap<(String, String), u64>,
    fetch_durations_ms: BTreeMap<(String, String), u64>,
    last_fetch_ts_ms: BTreeMap<(String, String), i64>,
    /// EXE-9: called when a database/metric error appears, changes, or
    /// clears — bounded by state change, never per tick.
    on_error: Option<ErrorCallback>,
    /// Last logged error signature per database/metric.
    error_signatures: BTreeMap<(String, String), Option<String>>,
    /// Scrape counter incremented by the HTTP side, read here at render.
    scrapes: Arc<AtomicU64>,
    /// Current count of dropped rows in the last render (assigned, not
    /// accumulated: it clears when the errors disappear).
    scrape_errors: u64,
    fetch_failure_count: u64,
    build_version: String,
    build_commit: String,
    start_time_ms: i64,
    /// Pre-rendered exposition served to scrapes.
    exposition: String,
}

impl<F: ExecutorFactory> Exporter<F> {
    /// Builds the exporter from a resolved preset and the shared scrape
    /// counter the HTTP side increments.
    #[must_use]
    pub fn new(
        factory: F,
        preset: Vec<PresetMetric>,
        build_version: impl Into<String>,
        build_commit: impl Into<String>,
        start_time_ms: i64,
        scrapes: Arc<AtomicU64>,
    ) -> Self {
        Self {
            factory,
            preset,
            instance_last_start_ms: BTreeMap::new(),
            instance_sets: BTreeMap::new(),
            per_db: BTreeMap::new(),
            on_error: None,
            error_signatures: BTreeMap::new(),
            fetch_errors: BTreeMap::new(),
            fetch_durations_ms: BTreeMap::new(),
            last_fetch_ts_ms: BTreeMap::new(),
            scrapes,
            scrape_errors: 0,
            fetch_failure_count: 0,
            build_version: build_version.into(),
            build_commit: build_commit.into(),
            start_time_ms,
            exposition: String::new(),
        }
    }

    /// The shared scrape counter for the HTTP side.
    #[must_use]
    pub fn scrape_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.scrapes)
    }

    /// The pre-rendered exposition body for scrapes.
    #[must_use]
    pub fn snapshot(&self) -> &str {
        &self.exposition
    }

    /// Installs the EXE-9 error-state callback.
    pub fn on_error(&mut self, callback: impl Fn(&str, &str, &str) + Send + Sync + 'static) {
        self.on_error = Some(Box::new(callback));
    }

    /// Reports an error state change; `None` text means the error cleared.
    #[allow(
        clippy::option_if_let_else,
        reason = "map_or with a unit closure trips the companion unused-return lint"
    )]
    fn report_error_state(&mut self, dbname: &str, metric: &str, signature: Option<String>) {
        let key = (dbname.to_owned(), metric.to_owned());
        // unchanged state, or a first success for a metric that never
        // errored: nothing to report
        let unchanged = match self.error_signatures.get(&key) {
            Some(prev) => prev == &signature,
            None => signature.is_none(),
        };
        if unchanged {
            return;
        }
        let text = signature.as_deref().unwrap_or("cleared").to_owned();
        self.error_signatures.insert(key, signature);
        if let Some(callback) = &self.on_error {
            callback(dbname, metric, &text);
        }
    }

    fn instance_up_interval(&self) -> u64 {
        self.preset
            .iter()
            .find(|m| m.name == INSTANCE_UP_METRIC)
            .map_or(INSTANCE_UP_INTERVAL_S, |m| m.interval_s)
    }

    /// Runs one pass over the discovered databases.
    ///
    /// `discovery_refreshed` re-enables metrics disabled by EXE-8; the
    /// collector sets it on the passes that follow its discovery cycle.
    /// The exposition renders with a stamp captured after the SQL, so
    /// samples keep their real execution timestamps and a just-fetched
    /// metric is never stale-or-future in its own pass.
    pub async fn run_pass(
        &mut self,
        discovered: &[String],
        discovery_refreshed: bool,
        now_ms: i64,
    ) {
        let started = std::time::Instant::now();
        // EXE-10: databases gone from discovery drop results, connections,
        // self-metric rows and instance-level fetches whose owning database
        // disappeared.
        self.per_db.retain(|name, _| discovered.contains(name));
        self.fetch_errors
            .retain(|(db, _), _| discovered.contains(db));
        self.fetch_durations_ms
            .retain(|(db, _), _| discovered.contains(db));
        self.last_fetch_ts_ms
            .retain(|(db, _), _| discovered.contains(db));
        self.error_signatures
            .retain(|(db, _), _| discovered.contains(db));
        self.instance_sets
            .retain(|_, (owner, _)| discovered.contains(owner));
        if discovery_refreshed {
            for state in self.per_db.values_mut() {
                state.disabled.clear();
            }
        }
        for dbname in discovered {
            if !self.per_db.contains_key(dbname) {
                self.per_db.insert(dbname.to_owned(), DbState::new());
            }
            self.pass_database(dbname, now_ms).await;
        }
        let names: Vec<String> = self.per_db.keys().cloned().collect();
        // Upstream keys its cache by the storage-resolved metric name, so
        // storage_name twins (e.g. db_size and db_size_approx) overwrite
        // each other instead of emitting duplicate series (prometheus.go
        // AddCacheEntry); both twins still fetch, last write wins.
        for (family, (_, set)) in &self.instance_sets {
            for dbname in &names {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    let interval_s = self
                        .preset
                        .iter()
                        .find(|m| m.def.exposed_name(&m.name) == family)
                        .map_or(INSTANCE_UP_INTERVAL_S, |m| m.interval_s);
                    state.cache.store(
                        family,
                        Entry {
                            interval_s,
                            set: relabel_dbname(set, dbname),
                        },
                    );
                }
            }
        }
        let elapsed_ms = u64::try_from(started.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .cast_signed();
        let pass_end_ms = now_ms.saturating_add(elapsed_ms);
        self.store_derived_instance_up(pass_end_ms);
        self.render(pass_end_ms);
    }

    /// Publishes the derived `instance_up` for every database.
    fn store_derived_instance_up(&mut self, stamp_ms: i64) {
        let interval = self.instance_up_interval();
        for (dbname, state) in &mut self.per_db {
            let changed = state.stored_up.is_some_and(|(up, _)| up != state.connected);
            let due_refresh = state
                .stored_up
                .is_none_or(|(_, stored_ms)| due(stored_ms, interval, stamp_ms));
            if changed || due_refresh {
                state.cache.store_instance_up(
                    instance_up_sample_set(dbname, state.connected, stamp_ms),
                    interval,
                );
                state.stored_up = Some((state.connected, stamp_ms));
            }
        }
    }

    async fn pass_database(&mut self, dbname: &str, now_ms: i64) {
        // Open the executor when missing; a failed open is a
        // connection-level failure.
        if self.per_db.get(dbname).is_some_and(|s| s.sql.is_none()) {
            let opened = self.factory.open_sql(dbname).await;
            if let Some(executor) = opened {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.sql = Some(executor);
                }
            } else if let Some(state) = self.per_db.get_mut(dbname) {
                state.connected = false;
            }
        }
        // The setup probe also establishes the facts on a new connection.
        let mut ran_query = false;
        let needs_facts = self.per_db.get(dbname).is_some_and(|s| s.facts.is_none());
        if needs_facts {
            ran_query = true; // the probe is an exchange
            self.probe(dbname).await;
        }
        let Some(facts) = self.per_db.get(dbname).and_then(|s| s.facts) else {
            return;
        };

        for metric in self.preset.clone() {
            if metric.name == INSTANCE_UP_METRIC
                || NOT_EXPOSED_METRICS.contains(&metric.name.as_str())
            {
                continue;
            }
            // node_status restricts against the recovery role read by the
            // connection setup probe.
            match metric.def.node_status {
                Some(NodeStatus::Primary) if facts.in_recovery => continue,
                Some(NodeStatus::Standby) if !facts.in_recovery => continue,
                _ => {}
            }
            if metric.def.is_instance_level {
                // CAT-9: once per interval on the first database able to run
                // it; the due check stays open until some database succeeds.
                let last = self
                    .instance_last_start_ms
                    .get(&metric.name)
                    .copied()
                    .unwrap_or(i64::MIN);
                if !due(last, metric.interval_s, now_ms) {
                    continue;
                }
                if self.try_run_instance_metric(dbname, &metric, now_ms).await {
                    self.instance_last_start_ms
                        .insert(metric.name.clone(), now_ms);
                }
            } else {
                let state_last = self
                    .per_db
                    .get(dbname)
                    .and_then(|s| s.last_metric_start_ms.get(&metric.name))
                    .copied()
                    .unwrap_or(i64::MIN);
                if !due(state_last, metric.interval_s, now_ms) {
                    continue;
                }
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state
                        .last_metric_start_ms
                        .insert(metric.name.clone(), now_ms);
                }
                ran_query = true;
                self.run_db_metric(dbname, &metric, now_ms).await;
            }
        }

        // When instance_up is due and no exchange ran this pass, the setup
        // select serves as the availability probe.
        let interval = self.instance_up_interval();
        let probe_due = self.per_db.get(dbname).is_some_and(|s| {
            s.stored_up
                .is_none_or(|(_, stored_ms)| due(stored_ms, interval, now_ms))
        });
        if !ran_query && probe_due {
            self.probe(dbname).await;
        }
    }

    /// Runs the setup select; success means the connection works.
    async fn probe(&mut self, dbname: &str) {
        let result = if let Some(state) = self.per_db.get_mut(dbname)
            && let Some(sql) = state.sql.as_mut()
        {
            sql.server_facts().await
        } else {
            return;
        };
        match result {
            Ok(facts) => {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.connected = true;
                    state.facts = Some(facts);
                }
            }
            Err(error) => {
                self.report_error_state(dbname, INSTANCE_UP_METRIC, Some(error.to_string()));
                if error.connection_lost {
                    self.drop_connection(dbname);
                }
            }
        }
    }

    /// Drops the executor (and with it the connection) after a
    /// connection-level failure.
    fn drop_connection(&mut self, dbname: &str) {
        if let Some(state) = self.per_db.get_mut(dbname) {
            state.sql = None;
            state.connected = false;
        }
    }

    /// Runs one instance-level fetch; true when SQL was actually attempted.
    async fn try_run_instance_metric(
        &mut self,
        dbname: &str,
        metric: &PresetMetric,
        now_ms: i64,
    ) -> bool {
        let Some(sql_text) = self.sql_for(dbname, metric) else {
            return false;
        };
        self.execute_and_store(dbname, metric, &sql_text, now_ms, true)
            .await;
        true
    }

    async fn run_db_metric(&mut self, dbname: &str, metric: &PresetMetric, now_ms: i64) {
        let Some(sql_text) = self.sql_for(dbname, metric) else {
            return;
        };
        self.execute_and_store(dbname, metric, &sql_text, now_ms, false)
            .await;
    }

    async fn execute_and_store(
        &mut self,
        dbname: &str,
        metric: &PresetMetric,
        sql_text: &str,
        now_ms: i64,
        instance_level: bool,
    ) {
        let timeout = metric.def.statement_timeout_seconds;
        let started = std::time::Instant::now();
        let result = self.query_with_retry(dbname, sql_text, timeout).await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.record_fetch(dbname, &metric.name, now_ms, duration_ms, &result);
        match result {
            Ok(result) => {
                self.report_error_state(dbname, &metric.name, None);
                let set = to_sample_set(
                    &result,
                    &metric.name,
                    metric.def.exposed_name(&metric.name),
                    &metric.def.gauges,
                    dbname,
                    now_ms,
                );
                let family = metric.def.exposed_name(&metric.name).to_owned();
                if instance_level {
                    self.instance_sets.insert(family, (dbname.to_owned(), set));
                } else if let Some(state) = self.per_db.get_mut(dbname) {
                    state.cache.store(
                        &family,
                        Entry {
                            interval_s: metric.interval_s,
                            set,
                        },
                    );
                }
            }
            Err(error) => self.handle_error(dbname, &metric.name, &error),
        }
    }

    /// One query exchange; on a connection-level failure, drops the
    /// connection, reconnects with the full setup, and retries once.
    async fn query_with_retry(
        &mut self,
        dbname: &str,
        sql_text: &str,
        timeout: Option<u64>,
    ) -> Result<crate::measurement::QueryResult, MetricError> {
        let first = self.query_once(dbname, sql_text, timeout).await;
        if !first.as_ref().is_err_and(|e| e.connection_lost) {
            return first;
        }
        // Reconnect: drop, reopen, full setup, then the same query once more.
        self.drop_connection(dbname);
        let reopened = self.factory.open_sql(dbname).await;
        let Some(executor) = reopened else {
            return first;
        };
        let mut executor = executor;
        let facts = executor.server_facts().await;
        match facts {
            Ok(facts) => {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.sql = Some(executor);
                    state.connected = true;
                    state.facts = Some(facts);
                }
                self.query_once(dbname, sql_text, timeout).await
            }
            Err(error) => {
                self.report_error_state(dbname, INSTANCE_UP_METRIC, Some(error.to_string()));
                Err(error)
            }
        }
    }

    async fn query_once(
        &mut self,
        dbname: &str,
        sql_text: &str,
        timeout: Option<u64>,
    ) -> Result<crate::measurement::QueryResult, MetricError> {
        let Some(state) = self.per_db.get_mut(dbname) else {
            return Err(MetricError::transport("database state disappeared"));
        };
        let Some(sql) = state.sql.as_mut() else {
            return Err(MetricError::transport("no exporter connection"));
        };
        let result = sql.execute(sql_text, timeout).await;
        match &result {
            Ok(_) => {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.connected = true;
                }
            }
            Err(error) if error.connection_lost => {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.connected = false;
                }
            }
            Err(_) => {}
        }
        result
    }

    fn record_fetch(
        &mut self,
        dbname: &str,
        metric: &str,
        now_ms: i64,
        duration_ms: u64,
        result: &Result<crate::measurement::QueryResult, MetricError>,
    ) {
        let key = (dbname.to_owned(), metric.to_owned());
        self.last_fetch_ts_ms.insert(key.clone(), now_ms);
        self.fetch_durations_ms.insert(key.clone(), duration_ms);
        if result.is_err() {
            *self.fetch_errors.entry(key).or_insert(0) += 1;
            self.fetch_failure_count += 1;
        }
    }

    fn sql_for(&self, dbname: &str, metric: &PresetMetric) -> Option<String> {
        let state = self.per_db.get(dbname)?;
        if state.disabled.contains(&metric.name) {
            return None;
        }
        let version = state.facts?.server_major_version;
        metric
            .def
            .sql_for_version(version)
            .and_then(crate::catalog::Sql::select)
            .map(str::to_owned)
    }

    /// EXE-8: missing objects disable the metric until discovery refresh;
    /// a lost connection drops the SQL executor for a fresh open next pass.
    fn handle_error(&mut self, dbname: &str, metric: &str, error: &MetricError) {
        self.report_error_state(dbname, metric, Some(error.to_string()));
        if error.missing_object()
            && let Some(state) = self.per_db.get_mut(dbname)
            && !state.disabled.iter().any(|m| m == metric)
        {
            state.disabled.push(metric.to_owned());
        }
        if error.connection_lost {
            self.drop_connection(dbname);
        }
    }

    /// Rebuilds the stored exposition body from the cache and self metrics.
    fn render(&mut self, now_ms: i64) {
        let mut sets: Vec<SampleSet> = Vec::new();
        // the gauge holds the current dropped-row count and clears when
        // the errors disappear
        let mut scrape_errors = 0_usize;
        for state in self.per_db.values() {
            for row in state.cache.snapshot(now_ms) {
                scrape_errors += row.entry.set.errors;
                sets.push(row.entry.set);
            }
        }
        self.scrape_errors = u64::try_from(scrape_errors).unwrap_or(u64::MAX);
        let mut out = crate::expose::expose_samples(sets.iter());
        out.push_str(&self.self_metrics_text());
        self.exposition = out;
    }

    fn self_metrics_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let family = |out: &mut String, name: &str, help: &str, kind: &str| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} {kind}");
        };
        family(
            &mut out,
            "kronika_build_info",
            "Collector build version and commit.",
            "gauge",
        );
        let _ = writeln!(
            out,
            "kronika_build_info{{commit=\"{}\",version=\"{}\"}} 1",
            self.build_commit, self.build_version
        );
        family(
            &mut out,
            "kronika_start_time_seconds",
            "Collector start time in seconds.",
            "gauge",
        );
        let _ = writeln!(
            out,
            "kronika_start_time_seconds {}",
            self.start_time_ms / 1000
        );
        family(
            &mut out,
            "kronika_pg_connected",
            "Last exporter connection verdict per database.",
            "gauge",
        );
        for (db, state) in &self.per_db {
            let _ = writeln!(
                out,
                "kronika_pg_connected{{database=\"{}\"}} {}",
                crate::expose::escape_label_value(db),
                u8::from(state.connected)
            );
        }
        family(
            &mut out,
            "pgwatch_exporter_total_scrapes",
            "Total scrape attempts.",
            "counter",
        );
        let _ = writeln!(
            out,
            "pgwatch_exporter_total_scrapes {}",
            self.scrapes.load(Ordering::Relaxed)
        );
        family(
            &mut out,
            "pgwatch_exporter_last_scrape_errors",
            "Last scrape error count for all monitored hosts / metrics.",
            "gauge",
        );
        let _ = writeln!(
            out,
            "pgwatch_exporter_last_scrape_errors {}",
            self.scrape_errors
        );
        family(
            &mut out,
            "pgwatch_exporter_total_scrape_failures",
            "Number of errors while executing metric queries.",
            "counter",
        );
        let _ = writeln!(
            out,
            "pgwatch_exporter_total_scrape_failures {}",
            self.fetch_failure_count
        );
        out.push_str(&self.fetch_self_metrics());
        out
    }

    /// Duration/timestamp/error rows per database and metric. They render
    /// even when the error map is empty: a healthy exporter still carries
    /// durations and timestamps.
    #[allow(
        clippy::cast_precision_loss,
        reason = "millisecond durations as fractional seconds lose nothing at these magnitudes"
    )]
    fn fetch_self_metrics(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let family = |out: &mut String, name: &str, help: &str, kind: &str| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} {kind}");
        };
        family(
            &mut out,
            "kronika_prometheus_fetch_errors_total",
            "Metric fetch errors per database and metric.",
            "counter",
        );
        for ((db, metric), count) in &self.fetch_errors {
            let _ = writeln!(
                out,
                "kronika_prometheus_fetch_errors_total{{database=\"{}\",metric=\"{}\"}} {count}",
                crate::expose::escape_label_value(db),
                crate::expose::escape_label_value(metric)
            );
        }
        family(
            &mut out,
            "kronika_prometheus_last_fetch_timestamp_seconds",
            "Last metric fetch completion time per database and metric.",
            "gauge",
        );
        for ((db, metric), ts) in &self.last_fetch_ts_ms {
            let _ = writeln!(
                out,
                "kronika_prometheus_last_fetch_timestamp_seconds{{database=\"{}\",metric=\"{}\"}} {}",
                crate::expose::escape_label_value(db),
                crate::expose::escape_label_value(metric),
                ts / 1000
            );
        }
        family(
            &mut out,
            "kronika_prometheus_fetch_duration_seconds",
            "Last metric fetch duration per database and metric.",
            "gauge",
        );
        for ((db, metric), ms) in &self.fetch_durations_ms {
            let _ = writeln!(
                out,
                "kronika_prometheus_fetch_duration_seconds{{database=\"{}\",metric=\"{}\"}} {}",
                crate::expose::escape_label_value(db),
                crate::expose::escape_label_value(metric),
                (*ms as f64) / 1000.0
            );
        }
        out
    }
}
/// Rebuilds `dbname` labels of an instance-level set for one database.
fn relabel_dbname(set: &SampleSet, dbname: &str) -> SampleSet {
    let mut set = set.clone();
    let owned = dbname.to_owned();
    for sample in &mut set.samples {
        if let Some(slot) = sample.labels.iter_mut().find(|(k, _)| k == "dbname") {
            slot.1.clone_from(&owned);
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, Gauges, Sql};
    use crate::measurement::{Column, QueryResult};
    use crate::typing::ColumnKind;
    use std::collections::{HashSet, VecDeque};
    use std::sync::Mutex;

    type SharedDb = Arc<Mutex<MockDb>>;

    struct MockDb {
        facts_result: Result<ServerFacts, MetricError>,
        sql_results: VecDeque<Result<QueryResult, MetricError>>,
        facts_calls: usize,
        sql_calls: Vec<String>,
    }

    struct MockSql {
        db: SharedDb,
    }

    impl SqlExecutor for MockSql {
        async fn server_facts(&mut self) -> Result<ServerFacts, MetricError> {
            let mut db = self.db.lock().expect("mock");
            db.facts_calls += 1;
            db.facts_result.clone()
        }

        async fn execute(
            &mut self,
            sql: &str,
            _statement_timeout_s: Option<u64>,
        ) -> Result<QueryResult, MetricError> {
            let mut db = self.db.lock().expect("mock");
            db.sql_calls.push(sql.to_owned());
            db.sql_results
                .pop_front()
                .unwrap_or_else(|| Ok(query_result()))
        }
    }

    #[derive(Default)]
    struct MockFactory {
        dbs: BTreeMap<String, SharedDb>,
        open_fails: HashSet<String>,
    }

    impl MockFactory {
        fn db(&self, name: &str) -> SharedDb {
            Arc::clone(self.dbs.get(name).expect("db registered"))
        }
    }

    impl ExecutorFactory for MockFactory {
        type Sql = MockSql;

        async fn open_sql(&mut self, dbname: &str) -> Option<MockSql> {
            self.dbs
                .get(dbname)
                .filter(|_| !self.open_fails.contains(dbname))
                .map(|db| MockSql { db: Arc::clone(db) })
        }
    }

    fn query_result() -> QueryResult {
        QueryResult {
            columns: vec![
                Column {
                    name: "epoch_ns".to_owned(),
                    kind: ColumnKind::Int,
                },
                Column {
                    name: "xact_commit".to_owned(),
                    kind: ColumnKind::Int,
                },
            ],
            rows: vec![vec![
                Some("1700000000500000000".to_owned()),
                Some("7".to_owned()),
            ]],
        }
    }

    fn basic_preset() -> Vec<PresetMetric> {
        Catalog::embedded()
            .expect("embedded catalog")
            .resolve_preset("basic")
            .expect("basic preset")
            .into_iter()
            .map(|(name, def, interval_s)| PresetMetric {
                name,
                def,
                interval_s,
            })
            .collect()
    }

    fn exporter(factory: MockFactory) -> Exporter<MockFactory> {
        Exporter::new(
            factory,
            basic_preset(),
            "1.2.4",
            "abc123",
            1_700_000_000_000,
            Arc::new(AtomicU64::new(0)),
        )
    }

    fn db(sql_results: Vec<Result<QueryResult, MetricError>>) -> SharedDb {
        db_with_facts(
            sql_results,
            Ok(ServerFacts {
                server_major_version: 16,
                in_recovery: false,
            }),
        )
    }

    fn db_with_facts(
        sql_results: Vec<Result<QueryResult, MetricError>>,
        facts: Result<ServerFacts, MetricError>,
    ) -> SharedDb {
        Arc::new(Mutex::new(MockDb {
            facts_result: facts,
            sql_results: VecDeque::from(sql_results),
            facts_calls: 0,
            sql_calls: Vec::new(),
        }))
    }

    async fn run(exporter: &mut Exporter<MockFactory>, dbs: &[&str], now_ms: i64) {
        let owned: Vec<String> = dbs.iter().map(|s| (*s).to_owned()).collect();
        exporter.run_pass(&owned, false, now_ms).await;
    }

    #[tokio::test]
    async fn instance_up_derives_from_connection_health() {
        let mut factory = MockFactory::default();
        factory.dbs.insert("h_app".to_owned(), db(vec![]));
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("kronika_pg_connected{database=\"h_app\"} 1"),
            "{text}"
        );

        // the next pass loses the connection: every query exchange fails at
        // the transport level and the reconnect setup probe is refused too
        let refused = || MetricError::transport("connect refused");
        let mock = e.factory.db("h_app");
        {
            let mut db = mock.lock().expect("mock");
            db.facts_result = Err(refused());
            db.sql_results = VecDeque::from([Err(refused()), Err(refused()), Err(refused())]);
        }
        run(&mut e, &["h_app"], 1_700_000_100_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 0"),
            "{text}"
        );
        assert!(
            text.contains("kronika_pg_connected{database=\"h_app\"} 0"),
            "{text}"
        );

        // recovery: the connection and queries work again
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").facts_result = Ok(ServerFacts {
            server_major_version: 16,
            in_recovery: false,
        });
        run(&mut e, &["h_app"], 1_700_000_200_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn sql_error_does_not_zero_instance_up() {
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![
                Ok(query_result()),
                Err(MetricError {
                    sqlstate: Some("XX000".to_owned()),
                    connection_lost: false,
                    message: "boom".to_owned(),
                }),
                Ok(query_result()),
            ]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("pgwatch_exporter_total_scrape_failures 1"),
            "{text}"
        );
        assert!(
            text.contains(
                "kronika_prometheus_fetch_errors_total{database=\"h_app\",metric=\"db_stats\"} 1"
            ),
            "{text}"
        );
    }

    #[tokio::test]
    async fn connection_loss_reconnects_and_retries_the_same_query_once() {
        let mut factory = MockFactory::default();
        // FIFO order at the first pass: db_size ok, db_stats transport loss,
        // wal ok; the retry consumes the next entry (a good result)
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![
                Ok(query_result()),
                Err(MetricError::transport("connection reset")),
                Ok(query_result()),
                Ok(query_result()),
            ]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        // the retry stored db_stats and the connection verdict stayed up
        assert!(
            text.contains("pgwatch_db_stats_xact_commit{dbname=\"h_app\"} 7"),
            "{text}"
        );
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        // the reconnect ran the setup probe again
        let facts_calls = e.factory.db("h_app").lock().expect("mock").facts_calls;
        assert_eq!(facts_calls, 2, "initial setup plus the reconnect setup");
        // db_stats was attempted twice: original and retry
        let db_stats_calls = e
            .factory
            .db("h_app")
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("pg_stat_database"))
            .count();
        assert_eq!(db_stats_calls, 2);
    }

    #[tokio::test]
    async fn facts_recovery_role_filters_node_status_metrics() {
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_standby".to_owned(),
            db_with_facts(
                vec![],
                Ok(ServerFacts {
                    server_major_version: 16,
                    in_recovery: true,
                }),
            ),
        );
        let mut e = exporter(factory);
        let mut preset: Vec<PresetMetric> = basic_preset()
            .into_iter()
            .filter(|m| m.name == INSTANCE_UP_METRIC || m.name == "db_stats")
            .collect();
        preset.push(PresetMetric {
            name: "primary_only".to_owned(),
            def: MetricDef {
                description: String::new(),
                sqls: BTreeMap::from([(14_u32, Sql::Select("select 1 as x".to_owned()))]),
                gauges: Gauges::All,
                is_instance_level: false,
                node_status: Some(NodeStatus::Primary),
                statement_timeout_seconds: None,
                storage_name: None,
            },
            interval_s: 60,
        });
        e.preset = preset;
        run(&mut e, &["h_standby"], 1_700_000_000_500).await;
        let sqls = e
            .factory
            .db("h_standby")
            .lock()
            .expect("mock")
            .sql_calls
            .clone();
        assert!(
            !sqls.iter().any(|s| s.contains("select 1 as x")),
            "primary-only metric skipped on a standby connection: {sqls:?}"
        );
        assert!(
            sqls.iter().any(|s| s.contains("pg_stat_database")),
            "unrestricted metric still ran: {sqls:?}"
        );
    }

    #[tokio::test]
    async fn intervals_gate_runs_against_start_time() {
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![Ok(query_result()), Ok(query_result())]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        run(&mut e, &["h_app"], 1_700_000_030_500).await;
        let calls = e.factory.db("h_app").lock().expect("mock").sql_calls.len();
        // all three basic SQL metrics ran once; 30s later nothing is due
        assert_eq!(calls, 3, "db_size, db_stats and wal once");
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        let calls = e.factory.db("h_app").lock().expect("mock").sql_calls.len();
        assert_eq!(calls, 5, "60s metrics due again, 300s db_size not yet");
    }

    #[tokio::test]
    async fn missing_object_disables_metric_until_discovery_refresh() {
        let missing = || {
            Err(MetricError {
                sqlstate: Some("42P01".to_owned()),
                connection_lost: false,
                message: "relation does not exist".to_owned(),
            })
        };
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![Ok(query_result()), missing(), Ok(query_result())]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        let db_stats_calls = e
            .factory
            .db("h_app")
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("pg_stat_database"))
            .count();
        assert_eq!(db_stats_calls, 1, "no retry after 42P01");

        let owned = vec!["h_app".to_owned()];
        e.run_pass(&owned, true, 1_700_000_122_500).await;
        let db_stats_calls = e
            .factory
            .db("h_app")
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("pg_stat_database"))
            .count();
        assert_eq!(db_stats_calls, 2, "re-enabled after discovery");
    }

    #[tokio::test]
    async fn failed_run_keeps_interval() {
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![Ok(query_result()), Ok(query_result())]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results =
            VecDeque::from([Err(MetricError::transport("hung"))]);
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        let len_before = mock.lock().expect("mock").sql_calls.len();
        run(&mut e, &["h_app"], 1_700_000_100_500).await;
        let len_after = mock.lock().expect("mock").sql_calls.len();
        assert_eq!(len_before, len_after, "error at 61s keeps the interval");
    }

    #[tokio::test]
    async fn database_disappearing_drops_state() {
        let mut factory = MockFactory::default();
        factory
            .dbs
            .insert("h_app".to_owned(), db(vec![Ok(query_result())]));
        factory
            .dbs
            .insert("h_gone".to_owned(), db(vec![Ok(query_result())]));
        let mut e = exporter(factory);
        run(&mut e, &["h_app", "h_gone"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(text.contains("dbname=\"h_gone\""), "{text}");
        run(&mut e, &["h_app"], 1_700_000_001_500).await;
        let text = e.snapshot().to_owned();
        assert!(!text.contains("dbname=\"h_gone\""), "{text}");
        assert!(text.contains("dbname=\"h_app\""), "{text}");
    }

    #[tokio::test]
    async fn disappearing_database_prunes_self_metrics() {
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![
                Ok(query_result()),
                Err(MetricError {
                    sqlstate: None,
                    connection_lost: false,
                    message: "boom".to_owned(),
                }),
                Ok(query_result()),
            ]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("kronika_prometheus_fetch_errors_total"),
            "{text}"
        );
        run(&mut e, &[], 1_700_000_061_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            !text.contains("database=\"h_app\""),
            "dead sample rows pruned with the database: {text}"
        );
    }

    #[tokio::test]
    async fn instance_level_metric_published_for_every_database() {
        let mut factory = MockFactory::default();
        factory
            .dbs
            .insert("h_a".to_owned(), db(vec![Ok(query_result())]));
        factory
            .dbs
            .insert("h_b".to_owned(), db(vec![Ok(query_result())]));
        let mut e = exporter(factory);
        run(&mut e, &["h_a", "h_b"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        // wal is instance-level in the embedded catalog: one execution on the
        // first database, rows under both dbname labels
        assert!(
            text.contains("pgwatch_wal_xact_commit{dbname=\"h_a\"} 7"),
            "{text}"
        );
        assert!(
            text.contains("pgwatch_wal_xact_commit{dbname=\"h_b\"} 7"),
            "{text}"
        );
        let a_wal = e
            .factory
            .db("h_a")
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("xlog_location_b"))
            .count();
        let b_wal = e
            .factory
            .db("h_b")
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("xlog_location_b"))
            .count();
        assert_eq!(
            (a_wal, b_wal),
            (1, 0),
            "wal ran once on the first database only"
        );
    }

    #[tokio::test]
    async fn storage_name_twins_share_one_family_last_write_wins() {
        // upstream keys its cache by the storage-resolved name: db_size and
        // db_size_approx both land on db_size and the later pass wins
        let mut factory = MockFactory::default();
        factory.dbs.insert("h_app".to_owned(), db(vec![]));
        let mut e = exporter(factory);
        let mut preset: Vec<PresetMetric> = basic_preset()
            .into_iter()
            .filter(|m| m.name == INSTANCE_UP_METRIC)
            .collect();
        for name in ["db_size", "db_size_approx"] {
            preset.push(PresetMetric {
                name: name.to_owned(),
                def: MetricDef {
                    description: String::new(),
                    sqls: BTreeMap::from([(14_u32, Sql::Select("select 1 as size_b".to_owned()))]),
                    gauges: Gauges::All,
                    is_instance_level: false,
                    node_status: None,
                    statement_timeout_seconds: None,
                    storage_name: Some("db_size".to_owned()),
                },
                interval_s: 60,
            });
        }
        e.preset = preset;
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let rows = e
            .snapshot()
            .lines()
            .filter(|l| l.starts_with("pgwatch_db_size_xact_commit"))
            .count();
        assert_eq!(rows, 1, "one series, not duplicated");
    }

    #[tokio::test]
    async fn fresh_sample_with_epoch_after_pass_start_is_exposed() {
        // the render stamp is captured after the SQL; an epoch slightly ahead
        // of the pass start is a real execution stamp and must not drop out
        let mut factory = MockFactory::default();
        factory.dbs.insert("h_app".to_owned(), db(vec![]));
        let mut e = exporter(factory);
        let ahead = QueryResult {
            columns: vec![
                Column {
                    name: "epoch_ns".to_owned(),
                    kind: ColumnKind::Int,
                },
                Column {
                    name: "xact_commit".to_owned(),
                    kind: ColumnKind::Int,
                },
            ],
            rows: vec![vec![
                Some("1700000000500000500".to_owned()),
                Some("7".to_owned()),
            ]],
        };
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results =
            VecDeque::from([Ok(query_result()), Ok(ahead), Ok(query_result())]);
        run(&mut e, &["h_app"], 1_700_000_000_400).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_db_stats_xact_commit{dbname=\"h_app\"} 7"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn idle_instance_up_probes_the_setup_select() {
        let mut factory = MockFactory::default();
        factory
            .dbs
            .insert("h_app".to_owned(), db(vec![Ok(query_result())]));
        let mut e = exporter(factory);
        // only instance_up in the preset: no metric SQL ever runs
        e.preset.retain(|m| m.name == INSTANCE_UP_METRIC);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        let calls = e.factory.db("h_app").lock().expect("mock").facts_calls;
        assert_eq!(calls, 1, "the setup select served as the probe");
        // next interval: probe again
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        let calls = e.factory.db("h_app").lock().expect("mock").facts_calls;
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn self_metrics_always_render_durations_and_timestamps() {
        // a healthy exporter still carries duration/timestamp rows: no early
        // return on an empty error map
        let mut factory = MockFactory::default();
        factory.dbs.insert(
            "h_app".to_owned(),
            db(vec![Ok(query_result()), Ok(query_result())]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("kronika_prometheus_fetch_duration_seconds{database=\"h_app\""),
            "{text}"
        );
        assert!(
            text.contains("kronika_prometheus_last_fetch_timestamp_seconds{database=\"h_app\""),
            "{text}"
        );
    }

    fn clean() -> QueryResult {
        QueryResult {
            columns: vec![
                Column {
                    name: "epoch_ns".to_owned(),
                    kind: ColumnKind::Int,
                },
                Column {
                    name: "v".to_owned(),
                    kind: ColumnKind::Int,
                },
            ],
            rows: vec![vec![
                Some("1700000000500000000".to_owned()),
                Some("1".to_owned()),
            ]],
        }
    }

    #[tokio::test]
    async fn last_scrape_errors_holds_the_current_count_and_clears() {
        // the gauge is assigned the current dropped-row count, never
        // accumulated per render
        let dup = || {
            Ok(QueryResult {
                columns: vec![
                    Column {
                        name: "epoch_ns".to_owned(),
                        kind: ColumnKind::Int,
                    },
                    Column {
                        name: "v".to_owned(),
                        kind: ColumnKind::Int,
                    },
                ],
                rows: vec![
                    vec![Some("1700000000500000000".to_owned()), Some("1".to_owned())],
                    vec![Some("1700000000500000000".to_owned()), Some("2".to_owned())],
                ],
            })
        };
        let mut factory = MockFactory::default();
        factory
            .dbs
            .insert("h_app".to_owned(), db(vec![dup(), Ok(clean())]));
        let mut e = exporter(factory);
        e.preset
            .retain(|m| m.name == INSTANCE_UP_METRIC || m.name == "db_stats");
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_exporter_last_scrape_errors 1"),
            "{text}"
        );
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        let text = e.snapshot().to_owned();
        assert!(
            text.contains("pgwatch_exporter_last_scrape_errors 0"),
            "the gauge holds the current count and clears: {text}"
        );
    }

    #[tokio::test]
    async fn error_states_reported_once_per_change() {
        // SQL-level failure: the connection stays usable, so no retry
        let hung = || MetricError {
            sqlstate: None,
            connection_lost: false,
            message: "hung".to_owned(),
        };
        let mut factory = MockFactory::default();
        factory
            .dbs
            .insert("h_app".to_owned(), db(vec![Ok(query_result())]));
        let mut e = exporter(factory);
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reports);
        e.on_error(move |db, metric, error| {
            sink.lock()
                .expect("sink")
                .push((db.to_owned(), metric.to_owned(), error.to_owned()));
        });
        // pass 1: db_stats fails (FIFO order: db_size ok, db_stats error, wal ok)
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results =
            VecDeque::from([Ok(query_result()), Err(hung()), Ok(query_result())]);
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        // pass 2: the same failure again — no new report (db_size is not due
        // at 61s, so only db_stats and wal consume results)
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results = VecDeque::from([Err(hung()), Ok(query_result())]);
        run(&mut e, &["h_app"], 1_700_000_061_500).await;
        // pass 3: healthy — one cleared report
        run(&mut e, &["h_app"], 1_700_000_122_500).await;
        let reports = reports.lock().expect("sink").clone();
        assert_eq!(
            reports,
            vec![
                ("h_app".to_owned(), "db_stats".to_owned(), "hung".to_owned()),
                (
                    "h_app".to_owned(),
                    "db_stats".to_owned(),
                    "cleared".to_owned()
                ),
            ],
            "EXE-9 reports state changes only"
        );
    }
}
