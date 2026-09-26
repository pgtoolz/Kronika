//! Prometheus `/metrics` endpoint: catalog startup, executors, HTTP, passes.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::TryStreamExt as _;
use kronika_prometheus::catalog::Catalog;
use kronika_prometheus::engine::{Exporter, PresetMetric};
use kronika_prometheus::executor::{
    ExecutorFactory, MetricError, PingExecutor, PingInfo, SqlExecutor,
};
use kronika_prometheus::measurement::{Column, QueryResult};
use kronika_prometheus::typing::{Cell, kind_for_oid};
use kronika_source_pg::{Pool, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::clock::unix_now_us;
use crate::config::Config;
use crate::logging::{LogLevel, field, log_event};

/// Fallback `statement_timeout` seconds (EXE-2 session default).
const DEFAULT_STATEMENT_TIMEOUT_S: u64 = 5;
/// Extra seconds beyond the statement timeout before the socket is dropped.
const HANG_GUARD_MARGIN_S: u64 = 5;
/// Session settings for exporter connections (EXE-2). Each is sent as its
/// own simple-protocol message: poolers such as `PgBouncer` reject
/// multi-statement packets, so nothing here may be packed together. They
/// ride every ping — idempotent, and they reattach to whatever connection
/// the pool hands out, including reconnects.
const SESSION_SETUP_STATEMENTS: [&str; 3] = [
    "SET application_name = 'kronika-prometheus'",
    "SET statement_timeout = '5s'",
    "SET lock_timeout = '100ms'",
];
/// The ping: one standalone statement whose success is the availability
/// check itself.
const PING_SQL: &str = "select current_setting('server_version_num')::int4, pg_is_in_recovery()";
/// Fresh recovery role for `node_status` filtering (CAT-8), one statement.
const RECOVERY_SQL: &str = "select pg_is_in_recovery()";

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

/// Loads the embedded catalog with overlays and resolves the preset.
pub(crate) fn load_catalog(config: &Config) -> Result<Catalog> {
    let mut catalog = Catalog::embedded().map_err(|e| anyhow::anyhow!("{e}"))?;
    for path in &config.prometheus_metrics {
        let overlay = read_overlay(path)?;
        catalog.overlay(overlay);
    }
    // Resolving validates preset and metric names up front (CFG-2).
    catalog
        .resolve_preset(&config.prometheus_preset)
        .map_err(|e| anyhow::anyhow!("--prometheus-preset: {e}"))?;
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

/// Runs one single-statement simple-protocol message and drains it.
async fn run_statement(
    session: kronika_source_pg::Session<'_>,
    sql: &str,
) -> Result<(), MetricError> {
    let mut stats = kronika_source_pg::query::QueryStats::default();
    let stream = session
        .simple_stream(sql, &mut stats)
        .await
        .map_err(|e| map_pg_error(&e))?;
    let mut stream = std::pin::pin!(stream);
    while stream
        .try_next()
        .await
        .map_err(|e| map_pg_error(&e))?
        .is_some()
    {
        // single statements return no rows here; nothing to collect
    }
    Ok(())
}

/// Ping executor on the dedicated per-database exporter connection.
/// One dedicated exporter connection shared by the ping and SQL executors
/// (EXE-2: one connection per database, separate from ordinary collection).
type SharedPool = Arc<Mutex<Pool>>;

struct ExporterPing {
    pool: SharedPool,
}

impl ExporterPing {
    #[allow(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the Session, which borrows the pool's client"
    )]
    async fn ping_inner(&self) -> Result<PingInfo, MetricError> {
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        let session = pool.session().await.map_err(|e| {
            let mut chain = format!("connect: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                chain.push_str(&format!("; caused by: {s}"));
                source = s.source();
            }
            MetricError::transport(chain)
        })?;
        let parse = async {
            for setup in SESSION_SETUP_STATEMENTS {
                run_statement(session, setup).await?;
            }
            let mut stats = kronika_source_pg::query::QueryStats::default();
            let stream = session
                .simple_stream(PING_SQL, &mut stats)
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
            let version = version.ok_or_else(|| MetricError::transport("ping returned no rows"))?;
            let in_recovery = in_recovery.unwrap_or_else(|| "f".to_owned());
            // server_version_num is e.g. 160015; the catalog keys on the major
            let version_num: u32 = version.parse().map_err(|_| {
                MetricError::transport(format!("bad server_version_num {version:?}"))
            })?;
            Ok(PingInfo {
                server_major_version: version_num / 10_000,
                in_recovery: in_recovery == "t",
            })
        };
        // A2: a hung transport must not stall the pass (and with it every
        // scrape). The ALREADY-HELD pool closes the socket on overrun —
        // never a second lock, the guard is still held here.
        match tokio::time::timeout(
            std::time::Duration::from_secs(DEFAULT_STATEMENT_TIMEOUT_S + HANG_GUARD_MARGIN_S),
            parse,
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                pool.close();
                Err(MetricError::transport(
                    "ping did not answer within statement_timeout + 5s; connection dropped",
                ))
            }
        }
    }
    #[allow(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the Session, which borrows the pool's client"
    )]
    async fn recovery_inner(&self) -> Result<bool, MetricError> {
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        let session = pool
            .session()
            .await
            .map_err(|e| MetricError::transport(format!("connect: {e}")))?;
        let read = async {
            let mut stats = kronika_source_pg::query::QueryStats::default();
            let stream = session
                .simple_stream(RECOVERY_SQL, &mut stats)
                .await
                .map_err(|e| map_pg_error(&e))?;
            let mut stream = std::pin::pin!(stream);
            let mut role = None;
            while let Some(message) = stream.try_next().await.map_err(|e| map_pg_error(&e))? {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    role = row.get(0).map(str::to_owned);
                }
            }
            Ok(role.is_some_and(|r| r == "t"))
        };
        // A2: same hang guard as the ping; the held pool closes on overrun
        match tokio::time::timeout(
            std::time::Duration::from_secs(DEFAULT_STATEMENT_TIMEOUT_S + HANG_GUARD_MARGIN_S),
            read,
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                pool.close();
                Err(MetricError::transport(
                    "recovery role read did not answer within statement_timeout + 5s; connection dropped",
                ))
            }
        }
    }
}

impl PingExecutor for ExporterPing {
    async fn ping(&mut self) -> Result<PingInfo, MetricError> {
        self.ping_inner().await
    }

    #[allow(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the Session, which borrows the pool's client"
    )]
    async fn recovery_role(&mut self) -> Result<bool, MetricError> {
        self.recovery_inner().await
    }
}

/// SQL executor on the shared exporter connection.
///
/// The per-metric `statement_timeout` is a plain `SET` in its own message
/// before the query (no transaction machinery, one statement per packet for
/// pooler compatibility). The hang guard closes the socket when no answer
/// arrives within the timeout plus five seconds — never a `CancelRequest`.
struct ExporterSql {
    pool: SharedPool,
}

impl SqlExecutor for ExporterSql {
    #[allow(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the Session, which borrows the pool's client"
    )]
    async fn execute(
        &mut self,
        sql: &str,
        statement_timeout_s: Option<u64>,
    ) -> Result<QueryResult, MetricError> {
        let timeout_s = statement_timeout_s.unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S);
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        let session = pool.session().await.map_err(|e| {
            let mut chain = format!("connect: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                chain.push_str(&format!("; caused by: {s}"));
                source = s.source();
            }
            MetricError::transport(chain)
        })?;
        // one statement per message: poolers reject multi-statement packets
        run_statement(session, &format!("SET statement_timeout = '{timeout_s}s'")).await?;
        let mut stats = kronika_source_pg::query::QueryStats::default();
        let stream = session
            .simple_stream(sql, &mut stats)
            .await
            .map_err(|e| map_pg_error(&e))?;
        let mut stream = std::pin::pin!(stream);
        let deadline = std::time::Duration::from_secs(timeout_s + HANG_GUARD_MARGIN_S);
        let collect = async {
            let mut columns: Option<Vec<Column>> = None;
            let mut rows: Vec<Vec<Cell>> = Vec::new();
            while let Some(message) = stream.try_next().await.map_err(|e| map_pg_error(&e))? {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                    let columns = columns.get_or_insert_with(|| {
                        row.columns()
                            .iter()
                            .map(|c| Column {
                                name: c.name().to_owned(),
                                kind: kind_for_oid(c.type_oid()),
                            })
                            .collect()
                    });
                    let cells = (0..columns.len())
                        .map(|i| row.get(i).map(str::to_owned))
                        .collect();
                    rows.push(cells);
                }
            }
            Ok::<QueryResult, MetricError>(QueryResult {
                columns: columns.unwrap_or_default(),
                rows,
            })
        };
        match tokio::time::timeout(deadline, collect).await {
            Ok(result) => result,
            // No answer within the guard: drop the socket. The server-side
            // statement_timeout usually ends the query first; this catches a
            // hung transport. Reconnect happens on the next pass.
            Err(_elapsed) => {
                // the collect future (and its stream) is dropped by the
                // the collect future (and its stream) is dropped by the
                // timeout; the already-held pool closes the socket. Never
                // re-lock: the guard is still held here and tokio mutexes
                // do not recurse, so a second lock would deadlock.
                pool.close();
                Err(MetricError::transport(
                    "metric query did not answer within statement_timeout + 5s; connection dropped",
                ))
            }
        }
    }
}

/// Maps a session-level error; the pool reopens dead connections itself, so
/// only connect failures mark the connection lost for the engine.
fn map_pg_error(error: &tokio_postgres::Error) -> MetricError {
    MetricError {
        sqlstate: error.code().map(|state| state.code().to_owned()),
        connection_lost: false,
        message: error.to_string(),
    }
}

/// Opens per-database exporter connections from the collector DSN.
struct CollectorFactory {
    primary: Pool,
    /// One shared pool (one connection) per database, keyed by label.
    per_db: BTreeMap<String, SharedPool>,
    /// `dbname` label prefix; labels are `<prefix>_<database>` (EXP-3).
    dbname_prefix: String,
}

impl CollectorFactory {
    /// The engine keys databases by exposition label; connections need the
    /// real database name. The prefix includes its trailing underscore, so
    /// stripping it once recovers the name.
    fn database_of<'a>(&'a self, label: &'a str) -> &'a str {
        label.strip_prefix(&self.dbname_prefix).unwrap_or(label)
    }
}

impl CollectorFactory {
    fn shared_pool(&mut self, label: &str) -> SharedPool {
        let database = self.database_of(label).to_owned();
        Arc::clone(
            self.per_db
                .entry(label.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(self.primary.on_database(&database)))),
        )
    }
}

impl ExecutorFactory for CollectorFactory {
    type Ping = ExporterPing;
    type Sql = ExporterSql;

    async fn open_ping(&mut self, dbname: &str) -> Option<ExporterPing> {
        Some(ExporterPing {
            pool: self.shared_pool(dbname),
        })
    }

    async fn open_sql(&mut self, dbname: &str) -> Option<ExporterSql> {
        Some(ExporterSql {
            pool: self.shared_pool(dbname),
        })
    }
}

/// The exporter plus its shared scrape snapshot.
pub(crate) struct PrometheusExporter {
    engine: Arc<Mutex<Exporter<CollectorFactory>>>,
    listener: TcpListener,
    /// `dbname` label prefix for discovered databases (EXP-3).
    dbname_prefix: String,
}

pub(crate) const fn enabled(config: &Config) -> bool {
    config.prometheus_listen.is_some()
}

/// Starts the exporter: catalog, listener task, and the shared engine.
pub(crate) async fn start(config: &Config) -> Result<Arc<PrometheusExporter>> {
    let catalog = load_catalog(config)?;
    let resolved = catalog
        .resolve_preset(&config.prometheus_preset)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let preset = resolved
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
    let primary = Pool::with_transport(dsn, transport)
        .map_err(|_e| anyhow::anyhow!("KRONIKA_PG_DSN is not a valid connection string"))?;
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
    let prefix = dbname_prefix(dsn);
    let mut engine = Exporter::new(
        CollectorFactory {
            primary,
            per_db: BTreeMap::new(),
            dbname_prefix: format!("{prefix}_"),
        },
        preset,
        env!("CARGO_PKG_VERSION"),
        "unknown",
        unix_now_us().unwrap_or_default() / 1000,
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
    Ok(Arc::new(PrometheusExporter {
        engine: Arc::new(Mutex::new(engine)),
        listener,
        dbname_prefix: prefix,
    }))
}

impl PrometheusExporter {
    /// One background pass on the collector tick.
    pub(crate) async fn run_pass(&self, databases: &[String], discovery_refreshed: bool) {
        // Discovery names become exposition dbname labels: <host>_<datname>.
        let labels: Vec<String> = databases
            .iter()
            .map(|db| format!("{}_{}", self.dbname_prefix, db))
            .collect();
        let now_ms = unix_now_us().map(|us| us / 1000).unwrap_or_default();
        self.engine
            .lock()
            .await
            .run_pass(&labels, discovery_refreshed, now_ms)
            .await;
    }

    /// Serves `/metrics` and `/health` until the process ends.
    #[allow(
        clippy::infinite_loop,
        reason = "the accept loop runs until the process exits"
    )]
    pub(crate) async fn serve(self: Arc<Self>) {
        loop {
            let Ok((mut socket, _peer)) = self.listener.accept().await else {
                continue;
            };
            let engine = Arc::clone(&self.engine);
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
                        // serves the body pre-rendered at the last pass end;
                        // the short lock never waits for SQL
                        let body = engine.lock().await.scrape();
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
