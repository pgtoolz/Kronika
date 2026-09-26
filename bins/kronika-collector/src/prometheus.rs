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
/// Session settings applied to every exporter connection (EXE-2).
const SESSION_SETUP_SQL: &str = "SET application_name = 'kronika-prometheus', \
     statement_timeout = '5s', lock_timeout = '100ms'";
/// One simple-protocol statement returning the server major version and
/// recovery role; its success is the availability check itself.
const PING_SQL: &str = "select current_setting('server_version_num')::int4, pg_is_in_recovery()";

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

/// Ping executor on the dedicated per-database exporter connection.
/// One dedicated exporter connection shared by the ping and SQL executors
/// (EXE-2: one connection per database, separate from ordinary collection).
type SharedPool = Arc<Mutex<Pool>>;

struct ExporterPing {
    pool: SharedPool,
    /// Generation whose session already received the `SET` setup.
    configured_generation: Option<u64>,
}

impl ExporterPing {
    async fn ping_inner(&mut self) -> Result<PingInfo, MetricError> {
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        let needs_setup = pool.generation() != self.configured_generation;
        let session = pool
            .session()
            .await
            .map_err(|e| MetricError::transport(format!("connect: {e}")))?;
        if needs_setup {
            let mut stats = kronika_source_pg::query::QueryStats::default();
            let setup = session
                .simple_stream(SESSION_SETUP_SQL, &mut stats)
                .await
                .map_err(|e| map_pg_error(&e))?;
            let mut setup = std::pin::pin!(setup);
            while setup
                .try_next()
                .await
                .map_err(|e| map_pg_error(&e))?
                .is_some()
            {
                // SET statements return no rows; nothing to collect
            }
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
        Ok(PingInfo {
            server_major_version: version.parse().unwrap_or(0),
            in_recovery: in_recovery == "t",
        })
    }
}

impl PingExecutor for ExporterPing {
    async fn ping(&mut self) -> Result<PingInfo, MetricError> {
        self.ping_inner().await
    }
}

/// SQL executor on the shared exporter connection.
///
/// The per-metric `statement_timeout` rides in the same simple-protocol
/// message as the query (plain `SET`; no transaction machinery), so one round
/// trip applies it. The hang guard closes the socket when no answer arrives
/// within the timeout plus five seconds — never a `CancelRequest`.
struct ExporterSql {
    pool: SharedPool,
}

impl SqlExecutor for ExporterSql {
    async fn execute(
        &mut self,
        sql: &str,
        statement_timeout_s: Option<u64>,
    ) -> Result<QueryResult, MetricError> {
        let timeout_s = statement_timeout_s.unwrap_or(DEFAULT_STATEMENT_TIMEOUT_S);
        let message = format!("SET statement_timeout = '{timeout_s}s';\n{sql}");
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        let session = pool
            .session()
            .await
            .map_err(|e| MetricError::transport(format!("connect: {e}")))?;
        let mut stats = kronika_source_pg::query::QueryStats::default();
        let stream = session
            .simple_stream(&message, &mut stats)
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
                                kind: kind_for_oid(u32::from(c.type_oid())),
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
    /// One shared pool (one connection) per database.
    per_db: BTreeMap<String, SharedPool>,
}

impl CollectorFactory {
    fn shared_pool(&mut self, dbname: &str) -> SharedPool {
        self.per_db
            .entry(dbname.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(self.primary.on_database(dbname))))
            .clone()
    }
}

impl ExecutorFactory for CollectorFactory {
    type Ping = ExporterPing;
    type Sql = ExporterSql;

    async fn open_ping(&mut self, dbname: &str) -> Option<ExporterPing> {
        Some(ExporterPing {
            pool: self.shared_pool(dbname),
            configured_generation: None,
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
    let engine = Exporter::new(
        CollectorFactory {
            primary,
            per_db: BTreeMap::new(),
        },
        preset,
        env!("CARGO_PKG_VERSION"),
        "unknown",
        unix_now_us().unwrap_or_default() / 1000,
    );
    Ok(Arc::new(PrometheusExporter {
        engine: Arc::new(Mutex::new(engine)),
        listener,
    }))
}

impl PrometheusExporter {
    /// One background pass on the collector tick.
    pub(crate) async fn run_pass(&self, databases: &[String], discovery_refreshed: bool) {
        let now_ms = unix_now_us().map(|us| us / 1000).unwrap_or_default();
        self.engine
            .lock()
            .await
            .run_pass(databases, discovery_refreshed, now_ms)
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
                        let now_ms = unix_now_us().unwrap_or_default() / 1000;
                        let body = engine.lock().await.scrape(now_ms);
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
