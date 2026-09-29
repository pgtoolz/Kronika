//! Prometheus `/metrics` endpoint: catalog startup, executors, HTTP, passes.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::TryStreamExt as _;
use kronika_prometheus::catalog::Catalog;
use kronika_prometheus::engine::{Exporter, PresetMetric, Published};
use kronika_prometheus::executor::{
    ExecutorFactory, MetricError, QueryOutcome, ServerFacts, SqlExecutor,
};
use kronika_prometheus::measurement::{Column, QueryResult};
use kronika_prometheus::typing::{Cell, kind_for_oid};
use kronika_source_pg::{Pool, Session, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::clock::unix_now_us;
use crate::config::Config;
use crate::logging::{LogLevel, field, log_event};

/// Fallback client deadline seconds when a metric sets no override.
const DEFAULT_STATEMENT_TIMEOUT_S: u64 = 5;
/// Extra seconds beyond the statement timeout before the socket is dropped.
const HANG_GUARD_MARGIN_S: u64 = 5;
/// The setup probe: one standalone statement run once per connection.
const SETUP_PROBE_SQL: &str =
    "select current_setting('server_version_num')::int4, pg_is_in_recovery()";
/// Startup `application_name` for exporter connections.
const EXPORTER_APPLICATION_NAME: &str = "kronika-prometheus";
/// How long a connection may take to deliver a complete request head.
const REQUEST_HEAD_DEADLINE: Duration = Duration::from_secs(10);
/// Upper bound of the request head; anything larger is malformed here.
const REQUEST_HEAD_LIMIT: usize = 8 * 1024;

/// Reads one bounded HTTP request head and returns the request path.
///
/// TCP splits a stream at arbitrary byte boundaries: `GET /met` may arrive
/// without `rics HTTP/1.1\r\n...`. Bytes accumulate until the head's
/// terminating blank line, the size limit, or EOF; a connection that never
/// finishes its head is dropped at the deadline.
async fn read_request_path<R: tokio::io::AsyncRead + Unpin>(
    socket: &mut R,
) -> Option<String> {
    let mut buffer: Vec<u8> = Vec::with_capacity(512);
    let mut chunk = [0_u8; 1024];
    loop {
        if buffer.len() >= REQUEST_HEAD_LIMIT || buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        let read = tokio::time::timeout(REQUEST_HEAD_DEADLINE, socket.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            break; // EOF: serve whatever complete request line arrived
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let request = String::from_utf8_lossy(&buffer);
    let path = request.split_whitespace().nth(1)?;
    (!path.is_empty()).then(|| path.split('?').next().unwrap_or_default().to_owned())
}

/// Exposition `dbname` label prefix: the DSN host, or the machine name for
/// Unix sockets and missing hosts (EXP-3).
fn dbname_prefix(dsn: &str) -> String {
    let config: tokio_postgres::Config = dsn.parse().expect("the DSN was validated at startup");
    match config.get_hosts().first() {
        Some(tokio_postgres::config::Host::Tcp(host)) => host.clone(),
        _ => std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|h| h.trim().to_owned())
            .unwrap_or_default(),
    }
}

/// Loads the embedded catalog with overlays.
pub(crate) fn load_catalog(config: &Config) -> Result<Catalog> {
    let mut catalog = Catalog::embedded().map_err(|e| anyhow::anyhow!("{e}"))?;
    for path in &config.prometheus_metrics {
        let overlay = read_overlay(path)?;
        catalog.overlay(overlay);
    }
    Ok(catalog)
}

fn read_overlay(path: &Path) -> Result<Catalog> {
    let mut files = Vec::new();
    let meta = std::fs::metadata(path).context("read the metrics overlay")?;
    if meta.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .context("read the metrics overlay directory")?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase),
                    Some(ref ext) if ext == "yaml" || ext == "yml"
                )
            })
            .collect();
        entries.sort();
        files = entries;
    } else {
        files.push(path.to_path_buf());
    }
    let mut merged = Catalog::default();
    for file in files {
        let text =
            std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
        let one = Catalog::from_yaml_str(&text, &file.display().to_string())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        merged.overlay(one);
    }
    Ok(merged)
}

/// Formats a connect failure with its full cause chain as one message.
fn connect_error<E: std::fmt::Display + std::error::Error>(error: E) -> MetricError {
    use std::fmt::Write as _;
    let mut chain = format!("connect: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(chain, "; caused by: {cause}");
        source = cause.source();
    }
    MetricError::transport(chain)
}

/// Maps a session-level error. A `PostgreSQL` error with a SQLSTATE is a
/// healthy server rejecting the statement; everything else — transport
/// reset, EOF, protocol breakage — means the connection is gone and the
/// engine must reconnect and retry (same criterion as the ordinary
/// collector's acquisition path).
fn map_pg_error(error: &tokio_postgres::Error) -> MetricError {
    MetricError {
        sqlstate: error.code().map(|state| state.code().to_owned()),
        connection_lost: error.as_db_error().is_none() || error.is_closed(),
        message: error.to_string(),
    }
}

/// The SQL executor: one per database, owning its dedicated connection.
///
/// Session configuration (`application_name`, `statement_timeout`,
/// `lock_timeout`) rides the startup packet, so the connection sends nothing
/// before its first real statement. Each exchange — the setup probe or one
/// metric SELECT — runs under a single client deadline; expiry closes the
/// socket through the owned pool. No `CancelRequest` is ever sent.
///
/// Facts are bound to the connection generation they were read on: the pool
/// replaces a closed or aged connection without telling the engine, so every
/// exchange re-checks the generation and reruns the setup probe when the
/// socket actually changed. A failover to another server version or recovery
/// role can never answer with the previous connection's facts.
struct ExporterSql {
    pool: Pool,
    facts: Option<(ServerFacts, u64)>,
}

impl ExporterSql {
    const PROBE_DEADLINE: Duration =
        Duration::from_secs(DEFAULT_STATEMENT_TIMEOUT_S + HANG_GUARD_MARGIN_S);

    /// Runs the setup probe on the session's connection and binds the facts
    /// to its generation.
    async fn probe_facts(session: &Session<'_>) -> Result<ServerFacts, MetricError> {
        let mut stats = kronika_source_pg::query::QueryStats::default();
        let stream = session
            .simple_stream(SETUP_PROBE_SQL, &mut stats)
            .await
            .map_err(|e| map_pg_error(&e))?;
        let mut stream = std::pin::pin!(stream);
        let mut version = None;
        let mut in_recovery = None;
        while let Some(message) = stream.try_next().await.map_err(|e| map_pg_error(&e))? {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                version = row.get(0).map(str::to_owned);
                in_recovery = row.get(1).map(str::to_owned);
            }
        }
        let version = version.ok_or_else(|| MetricError::transport("probe returned no rows"))?;
        let in_recovery = in_recovery.unwrap_or_else(|| "f".to_owned());
        // server_version_num is e.g. 160015; the catalog keys on the major
        let version_num: u32 = version.parse().map_err(|error| {
            MetricError::transport(format!("bad server_version_num {version:?}: {error}"))
        })?;
        Ok(ServerFacts {
            server_major_version: version_num / 10_000,
            in_recovery: in_recovery == "t",
        })
    }
}

impl SqlExecutor for ExporterSql {
    async fn server_facts(&mut self) -> Result<ServerFacts, MetricError> {
        // one deadline covers connect, the statement and the drain
        let probe = async {
            let session = self.pool.session().await.map_err(connect_error)?;
            let generation = session.generation();
            if let Some((facts, seen_on)) = self.facts
                && seen_on == generation
            {
                return Ok(facts);
            }
            let facts = Self::probe_facts(&session).await?;
            self.facts = Some((facts, generation));
            Ok(facts)
        };
        match tokio::time::timeout(Self::PROBE_DEADLINE, probe).await {
            Ok(result) => result,
            Err(_elapsed) => {
                // the cancelled future dropped the session; close the socket
                // through the pool this executor owns
                self.pool.close();
                Err(MetricError::transport(
                    "setup probe did not answer within the deadline; connection dropped",
                ))
            }
        }
    }

    async fn execute(
        &mut self,
        sql: &str,
        statement_timeout_s: Option<u64>,
    ) -> Result<QueryOutcome, MetricError> {
        let timeout_s = statement_timeout_s.unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S);
        // one deadline covers connect, a replacement setup probe when the
        // connection changed, the statement and the drain
        let collect = async {
            let session = self.pool.session().await.map_err(connect_error)?;
            let generation = session.generation();
            let facts_current = self
                .facts
                .as_ref()
                .is_some_and(|(_, seen_on)| *seen_on == generation);
            let refreshed_facts = if facts_current {
                None
            } else {
                let facts = Self::probe_facts(&session).await?;
                self.facts = Some((facts, generation));
                Some(facts)
            };
            let mut stats = kronika_source_pg::query::QueryStats::default();
            let stream = session
                .simple_stream(sql, &mut stats)
                .await
                .map_err(|e| map_pg_error(&e))?;
            let mut stream = std::pin::pin!(stream);
            let mut columns: Option<Vec<Column>> = None;
            let mut rows: Vec<Vec<Cell>> = Vec::new();
            while let Some(message) = stream.try_next().await.map_err(|e| map_pg_error(&e))? {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    let width = row.len();
                    let expected = columns.get_or_insert_with(|| {
                        row.columns()
                            .iter()
                            .map(|c| Column {
                                name: c.name().to_owned(),
                                kind: kind_for_oid(c.type_oid()),
                            })
                            .collect()
                    });
                    // a shape change inside one result set is a contract
                    // violation, not a panic: report it as an error
                    if width != expected.len() {
                        return Err(MetricError::transport(format!(
                            "unexpected row width {width} for {} columns",
                            expected.len()
                        )));
                    }
                    rows.push((0..width).map(|i| row.get(i).map(str::to_owned)).collect());
                }
            }
            Ok(QueryOutcome {
                result: QueryResult {
                    columns: columns.unwrap_or_default(),
                    rows,
                },
                refreshed_facts,
            })
        };
        match tokio::time::timeout(
            Duration::from_secs(timeout_s + HANG_GUARD_MARGIN_S),
            collect,
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                // the cancelled future dropped the session; close the socket
                // through the pool this executor owns
                self.pool.close();
                Err(MetricError::transport(
                    "metric query did not answer within statement_timeout + 5s; connection dropped",
                ))
            }
        }
    }
}

/// Opens per-database exporter connections from the collector DSN.
struct CollectorFactory {
    primary: Pool,
    /// `dbname` label prefix; labels are `<prefix>_<database>` (EXP-3).
    dbname_prefix: String,
}

impl CollectorFactory {
    /// The engine keys databases by exposition label; connections need the
    /// real database name. The prefix includes its trailing underscore, so
    /// stripping it once recovers the name.
    fn database_of<'a>(&self, label: &'a str) -> Option<&'a str> {
        label.strip_prefix(&self.dbname_prefix)
    }
}

impl ExecutorFactory for CollectorFactory {
    type Sql = ExporterSql;

    async fn open_sql(&mut self, dbname: &str) -> Option<ExporterSql> {
        let database = self.database_of(dbname)?;
        Some(ExporterSql {
            pool: self.primary.on_database(database),
            facts: None,
        })
    }
}

/// Message the collector tick leaves for the exporter task. Only the
/// latest pending message survives: a pass slower than the tick coalesces
/// the backlog into one run instead of queueing obsolete work, while the
/// discovery-refresh flag of every dropped message is preserved.
struct PassMessage {
    databases: Vec<String>,
    discovery_refreshed: bool,
}

/// The exporter endpoint: listener, pending pass, published snapshot.
pub(crate) struct PrometheusExporter {
    /// State captured after each pass; every scrape renders it against
    /// the current clock, so entries expire without a new pass.
    snapshot: Arc<std::sync::Mutex<Arc<Published>>>,
    scrapes: Arc<AtomicU64>,
    pending: Arc<std::sync::Mutex<Option<PassMessage>>>,
    wakes: Arc<tokio::sync::Notify>,
    listener: TcpListener,
    /// `dbname` label prefix; labels are `<prefix>_<database>` (EXP-3).
    dbname_prefix: String,
}

pub(crate) const fn enabled(config: &Config) -> bool {
    config.prometheus_listen.is_some()
}

/// Starts the exporter: catalog, engine task, listener.
///
/// The engine lives in its own task. The collector tick leaves discovery
/// results in a single coalescing slot and never awaits exporter SQL; the
/// HTTP side serves the snapshot published at the end of each pass.
pub(crate) async fn start(config: &Config) -> Result<Arc<PrometheusExporter>> {
    let catalog = load_catalog(config)?;
    let resolved = catalog
        .resolve_preset(&config.prometheus_preset)
        .map_err(|e| anyhow::anyhow!("--prometheus-preset: {e}"))?;
    let preset: Vec<PresetMetric> = resolved
        .into_iter()
        .map(|(name, def, interval_s)| PresetMetric {
            name,
            def,
            interval_s,
        })
        .collect();
    let transport = Transport::from_ca_file(config.pg_ssl_root_cert.as_deref())?;
    let dsn = config
        .pg_dsn
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--prometheus-listen requires --pg-dsn"))?;
    // The startup packet carries the largest timeout in the preset so the
    // server never kills a long metric early; tighter per-metric bounds are
    // the client deadlines of each exchange.
    let startup_timeout = preset
        .iter()
        .map(|m| {
            m.def
                .statement_timeout_seconds
                .unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S)
        })
        .max()
        .unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S);
    let primary = Pool::with_startup(
        dsn,
        transport,
        EXPORTER_APPLICATION_NAME,
        &format!("-c statement_timeout={startup_timeout}s -c lock_timeout=100ms"),
    )
    .map_err(|_e| anyhow::anyhow!("KRONIKA_PG_DSN is not a valid connection string"))?;
    let prefix = dbname_prefix(dsn);
    let mut engine = Exporter::new(
        CollectorFactory {
            primary,
            dbname_prefix: format!("{prefix}_"),
        },
        preset,
        env!("CARGO_PKG_VERSION"),
        "unknown",
        unix_now_us().unwrap_or_default() / 1000,
        Arc::new(AtomicU64::new(0)),
    );
    // EXE-9: one line per error state change, never per tick.
    engine.on_error(|database, metric, error| {
        log_event(
            LogLevel::Warn,
            "prometheus_metric_error",
            &[
                field("database", database),
                field("metric", metric),
                field("error", error),
            ],
        );
    });
    let scrapes = engine.scrape_counter();
    let snapshot = Arc::new(std::sync::Mutex::new(engine.published_snapshot()));
    let published = Arc::clone(&snapshot);
    let pending = Arc::new(std::sync::Mutex::new(None::<PassMessage>));
    let wakes = Arc::new(tokio::sync::Notify::new());
    let task_pending = Arc::clone(&pending);
    let task_wakes = Arc::clone(&wakes);
    tokio::spawn(async move {
        loop {
            task_wakes.notified().await;
            // drain what accumulated during the previous pass, coalescing
            // again: only the latest database list ever runs
            loop {
                let message = { task_pending.lock().expect("pass slot").take() };
                let Some(message) = message else {
                    break;
                };
                // the pass clock is read at execution, not at enqueue
                let now_ms = unix_now_us().map(|us| us / 1000).unwrap_or_default();
                engine
                    .run_pass(&message.databases, message.discovery_refreshed, now_ms)
                    .await;
                *published.lock().expect("snapshot lock") = engine.published_snapshot();
            }
        }
    });
    let addr: SocketAddr = config
        .prometheus_listen
        .expect("checked by config validation");
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind the Prometheus endpoint {addr}"))?;
    log_event(
        LogLevel::Info,
        "prometheus_listening",
        &[field("address", addr.to_string())],
    );
    Ok(Arc::new(PrometheusExporter {
        snapshot,
        scrapes,
        pending,
        wakes,
        listener,
        dbname_prefix: prefix,
    }))
}

/// Leaves one pass in the slot: an existing pending message is replaced
/// (its database list is obsolete), but a discovery refresh it flagged is
/// preserved so EXE-8 re-enables cannot be lost to coalescing.
fn coalesce_pass(slot: &mut Option<PassMessage>, message: PassMessage) {
    match slot {
        Some(pending) => {
            pending.discovery_refreshed |= message.discovery_refreshed;
            pending.databases = message.databases;
        }
        None => *slot = Some(message),
    }
}

impl PrometheusExporter {
    /// Hands one pass to the exporter task; never blocks on SQL. A pass
    /// still pending is replaced, keeping its discovery-refresh signal:
    /// slow passes must not pile up obsolete database lists.
    pub(crate) fn run_pass(&self, databases: &[String], discovery_refreshed: bool) {
        // Discovery names become exposition dbname labels: <host>_<db>.
        let labels: Vec<String> = databases
            .iter()
            .map(|db| format!("{}_{}", self.dbname_prefix, db))
            .collect();
        coalesce_pass(
            &mut self.pending.lock().expect("pass slot"),
            PassMessage {
                databases: labels,
                discovery_refreshed,
            },
        );
        self.wakes.notify_one();
    }

    /// Serves `/metrics` and `/health` until the process ends.
    ///
    /// `/metrics` increments the scrape counter and reads the published
    /// snapshot under a short lock; it never waits for SQL.
    #[allow(
        clippy::infinite_loop,
        reason = "the accept loop runs until the process exits"
    )]
    pub(crate) async fn serve(self: Arc<Self>) {
        loop {
            let Ok((mut socket, _peer)) = self.listener.accept().await else {
                continue;
            };
            let exporter = Arc::clone(&self);
            tokio::spawn(async move {
                let Some(path) = read_request_path(&mut socket).await else {
                    return;
                };
                let (status, content_type, body) = match path.as_str() {
                    "/metrics" => {
                        exporter.scrapes.fetch_add(1, Ordering::Relaxed);
                        // clone the Arc under a short lock, render outside
                        // it: per-entry expiry against the scrape clock
                        let published =
                            Arc::clone(&exporter.snapshot.lock().expect("snapshot lock"));
                        let now_ms = unix_now_us().map(|us| us / 1000).unwrap_or_default();
                        let body = published.render(now_ms);
                        ("200 OK", "text/plain; version=0.0.4; charset=utf-8", body)
                    }
                    "/health" => (
                        "200 OK",
                        "text/plain; version=0.0.4; charset=utf-8",
                        String::new(),
                    ),
                    _ => (
                        "404 Not Found",
                        "text/plain; version=0.0.4; charset=utf-8",
                        String::new(),
                    ),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                drop(socket.write_all(head.as_bytes()).await);
                drop(socket.write_all(body.as_bytes()).await);
                drop(socket.flush().await);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(databases: &[&str], refreshed: bool) -> PassMessage {
        PassMessage {
            databases: databases.iter().map(|s| (*s).to_owned()).collect(),
            discovery_refreshed: refreshed,
        }
    }

    #[tokio::test]
    async fn a_request_split_across_reads_reaches_its_endpoint() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, mut server) = tokio::io::duplex(256);
        let writer = tokio::spawn(async move {
            client.write_all(b"GET /met").await.expect("first fragment");
            tokio::time::sleep(Duration::from_millis(20)).await;
            client
                .write_all(b"rics HTTP/1.1\r\nHost: local\r\n\r\n")
                .await
                .expect("second fragment");
        });
        let path = read_request_path(&mut server)
            .await
            .expect("the split request is reassembled");
        writer.await.expect("writer");
        assert_eq!(path, "/metrics");
    }

    #[tokio::test]
    async fn a_query_string_is_stripped_and_garbage_has_no_path() {
        use tokio::io::AsyncWriteExt as _;
        let (mut client, mut server) = tokio::io::duplex(256);
        client
            .write_all(b"GET /health?probe=1 HTTP/1.1\r\n\r\n")
            .await
            .expect("request");
        let path = read_request_path(&mut server).await.expect("parsed");
        assert_eq!(path, "/health");
        let (mut client, mut server) = tokio::io::duplex(256);
        drop(client); // EOF with nothing sent
        assert!(read_request_path(&mut server).await.is_none());
    }

    #[test]
    fn slow_passes_coalesce_into_the_latest_message() {
        let mut slot = None;
        coalesce_pass(&mut slot, msg(&["h_a", "h_b"], false));
        coalesce_pass(&mut slot, msg(&["h_a"], false));
        coalesce_pass(&mut slot, msg(&["h_c"], false));
        let pending = slot.expect("a message stays pending");
        assert_eq!(pending.databases, vec!["h_c".to_owned()]);
        assert!(!pending.discovery_refreshed);
    }

    #[test]
    fn coalescing_never_drops_a_discovery_refresh() {
        let mut slot = None;
        coalesce_pass(&mut slot, msg(&["h_a"], true));
        coalesce_pass(&mut slot, msg(&["h_a"], false));
        let pending = slot.expect("a message stays pending");
        assert!(
            pending.discovery_refreshed,
            "the flag of a replaced message survives"
        );
    }
}
