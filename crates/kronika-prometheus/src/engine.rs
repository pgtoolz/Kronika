//! Exporter pass driver: per-database scheduling, ping, metric execution.
//!
//! One pass runs on the collector's tick: databases sequentially, one metric
//! at a time (EXE-6). Intervals compare against the previous run's start,
//! successful or not (EXE-7). `42P01`/`42883` errors disable a metric for a
//! database until the next discovery refresh (EXE-8); databases gone from
//! discovery are dropped with their state (EXE-10). `instance_up` reflects
//! only the ping, never metric SQL errors.

use std::collections::BTreeMap;

use crate::cache::{DbCache, Entry, INSTANCE_UP_INTERVAL_S};
use crate::catalog::{MetricDef, NOT_EXPOSED_METRICS, NodeStatus};
use crate::executor::{ExecutorFactory, MetricError, PingExecutor, PingInfo, SqlExecutor};
use crate::measurement::{INSTANCE_UP_METRIC, SampleSet, instance_up_sample_set, to_sample_set};
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

/// Per-database exporter state.
struct DbState<F: ExecutorFactory> {
    cache: DbCache,
    /// Whether the last ping succeeded.
    ping_up: bool,
    /// Start of the last ping, epoch ms.
    last_ping_start_ms: i64,
    /// Start of the last run of each metric, epoch ms (EXE-7).
    last_metric_start_ms: BTreeMap<String, i64>,
    /// Metrics disabled until the next discovery refresh (EXE-8).
    disabled: Vec<String>,
    /// Server facts from the last successful ping.
    info: Option<PingInfo>,
    ping: Option<F::Ping>,
    sql: Option<F::Sql>,
}

impl<F: ExecutorFactory> DbState<F> {
    fn new(dbname: &str) -> Self {
        Self {
            cache: DbCache::new(dbname),
            ping_up: false,
            last_ping_start_ms: i64::MIN,
            last_metric_start_ms: BTreeMap::new(),
            disabled: Vec::new(),
            info: None,
            ping: None,
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
    scrape_count: u64,
    scrape_errors: u64,
    /// Pre-rendered exposition served to scrapes (A3).
    exposition: String,
    fetch_failure_count: u64,
    build_version: String,
    build_commit: String,
    start_time_ms: i64,
}

impl<F: ExecutorFactory> Exporter<F> {
    /// Builds the exporter from a resolved preset.
    #[must_use]
    pub fn new(
        factory: F,
        preset: Vec<PresetMetric>,
        build_version: impl Into<String>,
        build_commit: impl Into<String>,
        start_time_ms: i64,
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
            scrape_count: 0,
            scrape_errors: 0,
            exposition: String::new(),
            fetch_failure_count: 0,
            build_version: build_version.into(),
            build_commit: build_commit.into(),
            start_time_ms,
        }
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
    pub async fn run_pass(
        &mut self,
        discovered: &[String],
        discovery_refreshed: bool,
        now_ms: i64,
    ) {
        // EXE-10: databases gone from discovery drop results, connections,
        // self-metric rows (EXE-10/D4) and instance-level fetches whose
        // owning database disappeared (D5).
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
        // Instance-level metrics run on the first database able to run them;
        // the fresh set is afterwards republished for every database.
        for dbname in discovered {
            if !self.per_db.contains_key(dbname) {
                self.per_db.insert(dbname.to_owned(), DbState::new(dbname));
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
        // A3: scrapes serve this pre-rendered body and never wait for SQL;
        // counters and freshness lag at most one collector tick.
        self.render(now_ms);
    }

    /// Rebuilds the stored exposition body from the cache and self metrics.
    fn render(&mut self, now_ms: i64) {
        let mut sets: Vec<SampleSet> = Vec::new();
        let mut scrape_errors = 0_usize;
        for state in self.per_db.values() {
            for row in state.cache.snapshot(now_ms) {
                scrape_errors += row.entry.set.errors;
                sets.push(row.entry.set);
            }
        }
        self.scrape_errors += u64::try_from(scrape_errors).unwrap_or(u64::MAX);
        let mut out = crate::expose::expose_samples(sets.iter());
        out.push_str(&self.self_metrics_text());
        self.exposition = out;
    }

    async fn pass_database(&mut self, dbname: &str, now_ms: i64) {
        let ping_interval = self.instance_up_interval();
        let ping_due = self
            .per_db
            .get(dbname)
            .is_some_and(|s| due(s.last_ping_start_ms, ping_interval, now_ms));
        if ping_due {
            self.run_ping(dbname, now_ms).await;
        }

        // Metric SQL needs server facts from a successful ping and a SQL
        // executor (type-OID patch); without either, this pass stops here
        // with instance_up and self metrics only.
        let Some(mut info) = self.per_db.get(dbname).and_then(|s| s.info) else {
            return;
        };
        // CAT-8: re-read the recovery role every pass it matters, instead of
        // relying on the last ping handshake.
        if self.preset.iter().any(|m| m.def.node_status.is_some()) {
            let role = if let Some(state) = self.per_db.get_mut(dbname)
                && let Some(ping) = state.ping.as_mut()
            {
                ping.recovery_role().await
            } else {
                Ok(info.in_recovery)
            };
            match role {
                Ok(in_recovery) => info.in_recovery = in_recovery,
                Err(error) => {
                    // reported under its own name; never disables a metric
                    // and never touches instance_up
                    self.report_error_state(dbname, "recovery_role", Some(error.to_string()));
                    if error.connection_lost
                        && let Some(state) = self.per_db.get_mut(dbname)
                    {
                        state.ping = None;
                    }
                }
            }
        }
        if self.per_db.get(dbname).is_some_and(|s| s.sql.is_none())
            && let Some(sql) = self.factory.open_sql(dbname).await
            && let Some(state) = self.per_db.get_mut(dbname)
        {
            state.sql = Some(sql);
        }
        if self.per_db.get(dbname).is_none_or(|s| s.sql.is_none()) {
            return;
        }

        for metric in self.preset.clone() {
            if metric.name == INSTANCE_UP_METRIC
                || NOT_EXPOSED_METRICS.contains(&metric.name.as_str())
            {
                continue;
            }
            // CAT-8: role restriction against the fresh recovery role.
            match metric.def.node_status {
                Some(NodeStatus::Primary) if info.in_recovery => continue,
                Some(NodeStatus::Standby) if !info.in_recovery => continue,
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
                self.run_db_metric(dbname, &metric, now_ms).await;
            }
        }
    }

    async fn run_ping(&mut self, dbname: &str, now_ms: i64) {
        if let Some(state) = self.per_db.get_mut(dbname) {
            state.last_ping_start_ms = now_ms;
        }
        if self.per_db.get(dbname).is_some_and(|s| s.ping.is_none())
            && let Some(ping) = self.factory.open_ping(dbname).await
            && let Some(state) = self.per_db.get_mut(dbname)
        {
            state.ping = Some(ping);
        }
        let interval = self.instance_up_interval();
        let result = match self.per_db.get_mut(dbname) {
            Some(state) => match state.ping.as_mut() {
                Some(ping) => ping.ping().await.map_err(Some),
                // Never connected: the database is down for the exporter.
                None => Err(None),
            },
            None => return,
        };
        match result {
            Ok(info) => {
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.ping_up = true;
                    state.info = Some(info);
                    state
                        .cache
                        .store_instance_up(instance_up_sample_set(dbname, true, now_ms), interval);
                }
            }
            Err(error) => {
                // Transport loss drops the connection; it reopens next ping.
                if error.as_ref().is_some_and(|e| e.connection_lost)
                    && let Some(state) = self.per_db.get_mut(dbname)
                {
                    state.ping = None;
                }
                if let Some(state) = self.per_db.get_mut(dbname) {
                    state.ping_up = false;
                    state
                        .cache
                        .store_instance_up(instance_up_sample_set(dbname, false, now_ms), interval);
                }
            }
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
        let Some(state) = self.per_db.get_mut(dbname) else {
            return;
        };
        let Some(sql) = state.sql.as_mut() else {
            return;
        };
        let started = std::time::Instant::now();
        let timeout = metric.def.statement_timeout_seconds;
        let result = sql.execute(sql_text, timeout).await;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let key = (dbname.to_owned(), metric.name.clone());
        self.last_fetch_ts_ms.insert(key.clone(), now_ms);
        self.fetch_durations_ms.insert(key.clone(), duration_ms);
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
            Err(error) => {
                *self.fetch_errors.entry(key).or_insert(0) += 1;
                self.fetch_failure_count += 1;
                self.handle_error(dbname, &metric.name, &error);
            }
        }
    }

    fn sql_for(&self, dbname: &str, metric: &PresetMetric) -> Option<String> {
        let state = self.per_db.get(dbname)?;
        if state.disabled.contains(&metric.name) {
            return None;
        }
        let version = state.info?.server_major_version;
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
        if error.connection_lost
            && let Some(state) = self.per_db.get_mut(dbname)
        {
            state.sql = None;
        }
    }

    /// Exposition text for one scrape: the body rendered at the end of the
    /// last pass, so scrapes never wait for SQL. Catalog samples, `kronika_`
    /// self metrics and the `pgwatch_exporter_*` rows; the render already
    /// filtered stale cache entries. Counters lag at most one tick.
    #[must_use]
    pub fn scrape(&mut self) -> String {
        self.scrape_count += 1;
        self.exposition.clone()
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "millisecond durations as fractional seconds lose nothing at these magnitudes"
    )]
    fn self_metrics_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
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
            "Last exporter ping outcome per database.",
            "gauge",
        );
        for (db, state) in &self.per_db {
            let _ = writeln!(
                out,
                "kronika_pg_connected{{database=\"{}\"}} {}",
                crate::expose::escape_label_value(db),
                u8::from(state.ping_up)
            );
        }
        self.push_exporter_rows(&mut out);
        out
    }

    /// The `pgwatch_exporter_*` compatibility trio plus fetch self metrics.
    #[allow(
        clippy::cast_precision_loss,
        reason = "millisecond durations as fractional seconds lose nothing at these magnitudes"
    )]
    fn push_exporter_rows(&self, out: &mut String) {
        use std::fmt::Write as _;
        family(
            out,
            "pgwatch_exporter_total_scrapes",
            "Total scrape attempts.",
            "counter",
        );
        let _ = writeln!(out, "pgwatch_exporter_total_scrapes {}", self.scrape_count);
        family(
            out,
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
            out,
            "pgwatch_exporter_total_scrape_failures",
            "Number of errors while executing metric queries.",
            "counter",
        );
        let _ = writeln!(
            out,
            "pgwatch_exporter_total_scrape_failures {}",
            self.fetch_failure_count
        );
        if self.fetch_errors.is_empty() {
            return;
        }
        family(
            out,
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
            out,
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
            out,
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
    }
}

/// Writes one `# HELP`/`# TYPE` header pair.
fn family(out: &mut String, name: &str, help: &str, kind: &str) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    type SharedDb = Arc<Mutex<MockDb>>;

    struct MockDb {
        ping_result: Result<PingInfo, MetricError>,
        recovery_result: Result<bool, MetricError>,
        ping_calls: usize,
        sql_results: VecDeque<Result<QueryResult, MetricError>>,
        sql_calls: Vec<String>,
    }

    struct MockPing {
        db: SharedDb,
    }

    impl PingExecutor for MockPing {
        async fn ping(&mut self) -> Result<PingInfo, MetricError> {
            let mut db = self.db.lock().expect("mock");
            db.ping_calls += 1;
            db.ping_result.clone()
        }

        async fn recovery_role(&mut self) -> Result<bool, MetricError> {
            self.db.lock().expect("mock").recovery_result.clone()
        }
    }

    struct MockSql {
        db: SharedDb,
    }

    impl SqlExecutor for MockSql {
        async fn execute(
            &mut self,
            sql: &str,
            _statement_timeout_s: Option<u64>,
        ) -> Result<QueryResult, MetricError> {
            let mut db = self.db.lock().expect("mock");
            db.sql_calls.push(sql.to_owned());
            db.sql_results.pop_front().unwrap_or_else(query_result)
        }
    }

    #[derive(Default)]
    struct MockFactory {
        dbs: BTreeMap<String, SharedDb>,
        with_sql: bool,
    }

    impl MockFactory {
        fn db(&self, name: &str) -> SharedDb {
            Arc::clone(self.dbs.get(name).expect("db registered"))
        }
    }

    impl ExecutorFactory for MockFactory {
        type Ping = MockPing;
        type Sql = MockSql;

        async fn open_ping(&mut self, dbname: &str) -> Option<MockPing> {
            self.dbs
                .get(dbname)
                .map(|db| MockPing { db: Arc::clone(db) })
        }

        async fn open_sql(&mut self, dbname: &str) -> Option<MockSql> {
            self.with_sql
                .then(|| {
                    self.dbs
                        .get(dbname)
                        .map(|db| MockSql { db: Arc::clone(db) })
                })
                .flatten()
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "scripted results share one Vec<Result<..>> type"
    )]
    fn query_result() -> Result<QueryResult, MetricError> {
        Ok(QueryResult {
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
                Some("1700000000000000000".to_owned()),
                Some("7".to_owned()),
            ]],
        })
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
        )
    }

    fn db(up: bool, sql_results: Vec<Result<QueryResult, MetricError>>) -> SharedDb {
        let sql_results = VecDeque::from(sql_results);
        Arc::new(Mutex::new(MockDb {
            ping_result: if up {
                Ok(PingInfo {
                    server_major_version: 16,
                    in_recovery: false,
                })
            } else {
                Err(MetricError::transport("connect refused"))
            },
            recovery_result: Ok(false),
            ping_calls: 0,
            sql_calls: Vec::new(),
            sql_results,
        }))
    }

    async fn run(exporter: &mut Exporter<MockFactory>, dbs: &[&str], now_ms: i64) {
        let owned: Vec<String> = dbs.iter().map(|s| (*s).to_owned()).collect();
        exporter.run_pass(&owned, false, now_ms).await;
    }

    #[tokio::test]
    async fn ping_drives_instance_up_and_pg_connected() {
        let mut factory = MockFactory::default();
        factory.dbs.insert("h_app".to_owned(), db(true, vec![]));
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        let text = e.scrape();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("kronika_pg_connected{database=\"h_app\"} 1"),
            "{text}"
        );

        // transport failure zeroes instance_up and pg_connected
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").ping_result = Err(MetricError::transport("refused"));
        run(&mut e, &["h_app"], 61_000).await;
        let text = e.scrape();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 0"),
            "{text}"
        );
        assert!(
            text.contains("kronika_pg_connected{database=\"h_app\"} 0"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn sql_error_does_not_zero_instance_up() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert(
            "h_app".to_owned(),
            db(
                true,
                vec![
                    query_result(),
                    Err(MetricError {
                        sqlstate: Some("XX000".to_owned()),
                        connection_lost: false,
                        message: "boom".to_owned(),
                    }),
                    query_result(),
                ],
            ),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        let text = e.scrape();
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
    async fn intervals_gate_runs_against_start_time() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert(
            "h_app".to_owned(),
            db(true, vec![query_result(), query_result()]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        run(&mut e, &["h_app"], 30_000).await;
        let mock = e.factory.db("h_app");
        let calls = mock.lock().expect("mock").sql_calls.len();
        // all three basic SQL metrics ran once; 30s later nothing is due
        assert_eq!(calls, 3, "db_size, db_stats and wal once");
        run(&mut e, &["h_app"], 61_000).await;
        let calls = mock.lock().expect("mock").sql_calls.len();
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
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert(
            "h_app".to_owned(),
            db(true, vec![query_result(), missing(), query_result()]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        run(&mut e, &["h_app"], 61_000).await;
        // 42P01 disabled db_stats; only wal ran on the second pass
        let mock = e.factory.db("h_app");
        let db_stats_calls = mock
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("pg_stat_database"))
            .count();
        assert_eq!(db_stats_calls, 1, "no retry after 42P01");

        // a discovery refresh re-enables it
        let owned = vec!["h_app".to_owned()];
        e.run_pass(&owned, true, 122_000).await;
        let mock = e.factory.db("h_app");
        let db_stats_calls = mock
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
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert(
            "h_app".to_owned(),
            db(true, vec![query_result(), query_result()]),
        );
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        // replace remaining results with an error: the next due run fails,
        // but the one after still waits a full interval from its start.
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results =
            VecDeque::from([Err(MetricError::transport("hung"))]);
        run(&mut e, &["h_app"], 61_000).await;
        let mock = e.factory.db("h_app");
        let len_before = mock.lock().expect("mock").sql_calls.len();
        run(&mut e, &["h_app"], 100_000).await;
        let len_after = mock.lock().expect("mock").sql_calls.len();
        assert_eq!(len_before, len_after, "error at 61s keeps the interval");
    }

    #[tokio::test]
    async fn database_disappearing_drops_state() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory
            .dbs
            .insert("h_app".to_owned(), db(true, vec![query_result()]));
        factory
            .dbs
            .insert("h_gone".to_owned(), db(true, vec![query_result()]));
        let mut e = exporter(factory);
        run(&mut e, &["h_app", "h_gone"], 0).await;
        let text = e.scrape();
        assert!(text.contains("dbname=\"h_gone\""), "{text}");
        run(&mut e, &["h_app"], 1).await;
        let text = e.scrape();
        assert!(!text.contains("dbname=\"h_gone\""), "{text}");
        assert!(text.contains("dbname=\"h_app\""), "{text}");
    }

    #[tokio::test]
    async fn instance_level_metric_published_for_every_database() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory
            .dbs
            .insert("h_a".to_owned(), db(true, vec![query_result()]));
        factory
            .dbs
            .insert("h_b".to_owned(), db(true, vec![query_result()]));
        let mut e = exporter(factory);
        // the pass runs at the scripted epoch so the pre-rendered body
        // carries fresh (non-stale) rows
        run(&mut e, &["h_a", "h_b"], 1_700_000_000_500).await;
        let text = e.scrape();
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
        let a = e.factory.db("h_a");
        let b = e.factory.db("h_b");
        let a_wal = a
            .lock()
            .expect("mock")
            .sql_calls
            .iter()
            .filter(|sql| sql.contains("xlog_location_b"))
            .count();
        let b_wal = b
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
    async fn node_status_filters_by_recovery_role() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory
            .dbs
            .insert("h_standby".to_owned(), db(true, vec![query_result()]));
        let mut e = exporter(factory);
        // standby in recovery: primary-only metrics (db_size is not; wal is
        // instance-level and not restricted) — use a preset with a
        // node_status metric instead
        let mut preset = basic_preset();
        preset.retain(|m| m.name == "instance_up" || m.name == "db_stats");
        preset.push(PresetMetric {
            name: "reco_add_index".to_owned(),
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
        let mock = e.factory.db("h_standby");
        mock.lock().expect("mock").ping_result = Ok(PingInfo {
            server_major_version: 16,
            in_recovery: true,
        });
        // the fresh per-pass role agrees with the handshake
        mock.lock().expect("mock").recovery_result = Ok(true);
        e.preset = preset;
        run(&mut e, &["h_standby"], 0).await;
        let sqls = e
            .factory
            .db("h_standby")
            .lock()
            .expect("mock")
            .sql_calls
            .clone();
        assert!(
            !sqls.iter().any(|s| s.contains("select 1 as x")),
            "primary-only metric skipped on standby: {sqls:?}"
        );
        assert!(
            sqls.iter().any(|s| s.contains("pg_stat_database")),
            "unrestricted metric still ran: {sqls:?}"
        );
    }

    #[tokio::test]
    async fn fresh_recovery_role_overrides_the_handshake() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory
            .dbs
            .insert("h_db".to_owned(), db(true, vec![query_result()]));
        let mut e = exporter(factory);
        // the handshake says primary and the fresh per-pass read says
        // standby: a primary-only metric must be skipped anyway
        let mut preset: Vec<PresetMetric> = basic_preset()
            .into_iter()
            .filter(|m| m.name == "instance_up" || m.name == "db_stats")
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
        let mock = e.factory.db("h_db");
        mock.lock().expect("mock").recovery_result = Ok(true);
        e.preset = preset;
        run(&mut e, &["h_db"], 0).await;
        let sqls = e.factory.db("h_db").lock().expect("mock").sql_calls.clone();
        assert!(
            !sqls.iter().any(|s| s.contains("select 1 as x")),
            "fresh standby role skips the primary-only metric: {sqls:?}"
        );
    }

    #[tokio::test]
    async fn error_states_reported_once_per_change() {
        let transport = || MetricError::transport("hung");
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory
            .dbs
            .insert("h_app".to_owned(), db(true, vec![query_result()]));
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
            VecDeque::from([query_result(), Err(transport()), query_result()]);
        run(&mut e, &["h_app"], 0).await;
        // pass 2: the same failure again — no new report (db_size is not
        // due at 61s, so only db_stats and wal consume results)
        let mock = e.factory.db("h_app");
        mock.lock().expect("mock").sql_results = VecDeque::from([Err(transport()), query_result()]);
        run(&mut e, &["h_app"], 61_000).await;
        // pass 3: healthy — one cleared report
        run(&mut e, &["h_app"], 122_000).await;
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

    #[tokio::test]
    async fn storage_name_twins_share_one_family_last_write_wins() {
        // upstream keys its cache by the storage-resolved name: db_size and
        // db_size_approx both land on db_size and the later pass wins
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert("h_app".to_owned(), db(true, vec![]));
        let mut e = exporter(factory);
        let mut preset: Vec<PresetMetric> = basic_preset()
            .into_iter()
            .filter(|m| m.name == "instance_up")
            .collect();
        for (name, size) in [("db_size", "11"), ("db_size_approx", "22")] {
            preset.push(PresetMetric {
                name: name.to_owned(),
                def: MetricDef {
                    description: String::new(),
                    sqls: BTreeMap::from([(
                        14_u32,
                        Sql::Select(format!("select 1 as size_b where '{size}' = v")),
                    )]),
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
        // pass at the scripted epoch so the rendered body carries fresh rows
        run(&mut e, &["h_app"], 1_700_000_000_500).await;
        let text = e.scrape();
        let rows = text
            .lines()
            .filter(|l| l.starts_with("pgwatch_db_size_xact_commit"))
            .count();
        assert_eq!(rows, 1, "one series, not duplicated: {text}");
    }

    #[tokio::test]
    async fn disappearing_database_prunes_self_metrics() {
        let mut factory = MockFactory {
            with_sql: true,
            ..MockFactory::default()
        };
        factory.dbs.insert(
            "h_app".to_owned(),
            db(true, vec![query_result(), query_result(), query_result()]),
        );
        let mock_db = factory.dbs.get("h_app").cloned().unwrap();
        mock_db.lock().expect("mock").sql_results = VecDeque::from([
            query_result(),
            Err(MetricError::transport("boom")),
            query_result(),
        ]);
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        let text = e.scrape();
        assert!(
            text.contains("kronika_prometheus_fetch_errors_total"),
            "{text}"
        );
        run(&mut e, &[], 61_000).await;
        let text = e.scrape();
        assert!(
            !text.contains("kronika_prometheus_"),
            "dead rows pruned with the database: {text}"
        );
    }

    #[tokio::test]
    async fn without_sql_executor_only_ping_and_self_metrics() {
        let mut factory = MockFactory::default();
        factory.dbs.insert("h_app".to_owned(), db(true, vec![]));
        let mut e = exporter(factory);
        run(&mut e, &["h_app"], 0).await;
        let text = e.scrape();
        assert!(
            text.contains("pgwatch_instance_up{dbname=\"h_app\"} 1"),
            "{text}"
        );
        assert!(!text.contains("pgwatch_wal_"), "{text}");
        assert!(!text.contains("pgwatch_db_stats_"), "{text}");
        // the scrape count in the pre-rendered body lags one tick (A3)
        assert!(text.contains("pgwatch_exporter_total_scrapes 0"), "{text}");
        assert!(
            text.contains("kronika_build_info{commit=\"abc123\",version=\"1.2.4\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("kronika_start_time_seconds 1700000000"),
            "{text}"
        );
    }
}
