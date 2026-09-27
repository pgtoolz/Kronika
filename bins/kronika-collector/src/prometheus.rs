//! Prometheus `/metrics` endpoint: catalog startup, executors, HTTP, passes.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::TryStreamExt as _;
use kronika_prometheus::catalog::Catalog;
use kronika_prometheus::engine::{Exporter, PresetMetric};
use kronika_prometheus::executor::{ExecutorFactory, MetricError, ServerFacts, SqlExecutor};
use kronika_prometheus::measurement::{Column, QueryResult};
use kronika_prometheus::typing::{Cell, kind_for_oid};
use kronika_source_pg::{Pool, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

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

/// Maps a session-level error; the pool reopens dead connections itself, so
/// only the deadline expiry and closed transports mark the connection lost.
fn map_pg_error(error: &tokio_postgres::Error) -> MetricError {
    MetricError {
        sqlstate: error.code().map(|state| state.code().to_owned()),
        connection_lost: false,
        message: error.to_string(),
    }
}

/// The SQL executor: one per database, owning its dedicated connection.
///
/// Session configuration (`application_name`, `statement_timeout`,
/// `lock_timeout`) rides the startup packet, so the connection sends nothing
/// before its first real statement. Each exchange — the setup probe or one
/// metric SELECT — runs under a single client deadline; expiry closes the
/// socket through the held pool guard. No `CancelRequest` is ever sent.
struct ExporterSql {
    pool: Pool,
    facts: Option<ServerFacts>,
}

impl ExporterSql {
    const PROBE_DEADLINE: Duration =
        Duration::from_secs(DEFAULT_STATEMENT_TIMEOUT_S + HANG_GUARD_MARGIN_S);
}

impl SqlExecutor for ExporterSql {
    async fn server_facts(&mut self) -> Result<ServerFacts, MetricError> {
        // one deadline covers connect, the statement and the drain
        let probe = async {
            let session = self.pool.session().await.map_err(connect_error)?;
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
            let version =
                version.ok_or_else(|| MetricError::transport("probe returned no rows"))?;
            let in_recovery = in_recovery.unwrap_or_else(|| "f".to_owned());
            // server_version_num is e.g. 160015; the catalog keys on the major
            let version_num: u32 = version.parse().map_err(|error| {
                MetricError::transport(format!("bad server_version_num {version:?}: {error}"))
            })?;
            Ok(ServerFacts {
                server_major_version: version_num / 10_000,
                in_recovery: in_recovery == "t",
            })
        };
        match tokio::time::timeout(Self::PROBE_DEADLINE, probe).await {
            Ok(result) => result.inspect(|facts| self.facts = Some(*facts)),
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
    ) -> Result<QueryResult, MetricError> {
        let timeout_s = statement_timeout_s.unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S);
        // one deadline covers connect, the statement and the drain
        let collect = async {
            let session = self.pool.session().await.map_err(connect_error)?;
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
            Ok(QueryResult {
                columns: columns.unwrap_or_default(),
                rows,
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

/// Message the collector tick sends to the exporter task.
struct PassMessage {
    databases: Vec<String>,
    discovery_refreshed: bool,
    now_ms: i64,
}

/// The exporter endpoint: listener, pass channel, published snapshot.
pub(crate) struct PrometheusExporter {
    snapshot: Arc<std::sync::Mutex<String>>,
    scrapes: Arc<AtomicU64>,
    passes: mpsc::UnboundedSender<PassMessage>,
    listener: TcpListener,
}

pub(crate) const fn enabled(config: &Config) -> bool {
    config.prometheus_listen.is_some()
}

/// Starts the exporter: catalog, engine task, listener.
///
/// The engine lives in its own task. The collector tick sends discovery
/// results over an unbounded channel and never awaits exporter SQL; the
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
    let snapshot = Arc::new(std::sync::Mutex::new(String::new()));
    let published = Arc::clone(&snapshot);
    let (passes, mut inbox) = mpsc::unbounded_channel::<PassMessage>();
    tokio::spawn(async move {
        while let Some(message) = inbox.recv().await {
            engine
                .run_pass(
                    &message.databases,
                    message.discovery_refreshed,
                    message.now_ms,
                )
                .await;
            let mut published = published.lock().expect("snapshot lock");
            published.clear();
            published.push_str(engine.snapshot());
            drop(published);
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
        passes,
        listener,
    }))
}

impl PrometheusExporter {
    /// Hands one pass to the exporter task; never blocks on SQL.
    pub(crate) fn run_pass(&self, databases: &[String], discovery_refreshed: bool) {
        let now_ms = unix_now_us().map(|us| us / 1000).unwrap_or_default();
        drop(self.passes.send(PassMessage {
            // Discovery names become exposition dbname labels: <host>_<db>.
            databases: databases.to_vec(),
            discovery_refreshed,
            now_ms,
        }));
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
                let mut buffer = [0_u8; 2048];
                let read = match socket.read(&mut buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default();
                let (status, content_type, body) = match path {
                    "/metrics" => {
                        exporter.scrapes.fetch_add(1, Ordering::Relaxed);
                        let body = exporter.snapshot.lock().expect("snapshot lock").clone();
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
