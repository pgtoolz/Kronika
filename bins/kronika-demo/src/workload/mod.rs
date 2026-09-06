//! Optional `PostgreSQL` workload enabled by `KRONIKA_DEMO_WORKLOAD_DSN`.

mod dml;
mod events;
mod locks;
mod naming;
mod plans;
mod schema;
mod vacuum;

use anyhow::{Context, Result};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Runtime};
use tokio_postgres::{Client, Config, NoTls};

#[derive(Clone)]
pub(crate) struct WorkloadConfig {
    /// DML connection, normally through `PgBouncer`.
    pub(crate) dsn: String,
    /// Direct connection for session-scoped settings.
    pub(crate) direct_dsn: String,
    pub(crate) schemas: u32,
    pub(crate) tables_per_schema: u32,
    pub(crate) ddl_concurrency: u32,
    pub(crate) sessions: u32,
    pub(crate) transactions_per_second: u32,
    pub(crate) max_orders: u32,
    pub(crate) lock_chains: u32,
    pub(crate) lock_chain_depth: u32,
    pub(crate) lock_hold_ms: u64,
    pub(crate) lock_round_interval_s: u64,
    pub(crate) event_round_interval_s: u64,
    pub(crate) plan_rows: u32,
    pub(crate) plan_workers: u32,
    pub(crate) plan_baseline_s: u64,
    pub(crate) plan_regression_s: u64,
    pub(crate) plan_round_interval_s: u64,
    pub(crate) vacuum_rows: u32,
    pub(crate) vacuum_round_interval_s: u64,
    pub(crate) vacuum_statement_timeout_s: u64,
}

impl fmt::Debug for WorkloadConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkloadConfig")
            .field("dsn", &"[redacted]")
            .field("direct_dsn", &"[redacted]")
            .field("schemas", &self.schemas)
            .field("tables_per_schema", &self.tables_per_schema)
            .field("ddl_concurrency", &self.ddl_concurrency)
            .field("sessions", &self.sessions)
            .field("transactions_per_second", &self.transactions_per_second)
            .field("max_orders", &self.max_orders)
            .field("lock_chains", &self.lock_chains)
            .field("lock_chain_depth", &self.lock_chain_depth)
            .field("lock_hold_ms", &self.lock_hold_ms)
            .field("lock_round_interval_s", &self.lock_round_interval_s)
            .field("event_round_interval_s", &self.event_round_interval_s)
            .field("plan_rows", &self.plan_rows)
            .field("plan_workers", &self.plan_workers)
            .field("plan_baseline_s", &self.plan_baseline_s)
            .field("plan_regression_s", &self.plan_regression_s)
            .field("plan_round_interval_s", &self.plan_round_interval_s)
            .field("vacuum_rows", &self.vacuum_rows)
            .field("vacuum_round_interval_s", &self.vacuum_round_interval_s)
            .field(
                "vacuum_statement_timeout_s",
                &self.vacuum_statement_timeout_s,
            )
            .finish()
    }
}

fn env_u32(key: &str, default: u32) -> Result<u32> {
    match std::env::var(key) {
        Ok(raw) => raw
            .trim()
            .parse()
            .with_context(|| format!("{key}={raw:?} is not a u32")),
        Err(_unset) => Ok(default),
    }
}

fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => raw
            .trim()
            .parse()
            .with_context(|| format!("{key}={raw:?} is not a u64")),
        Err(_unset) => Ok(default),
    }
}

fn required_direct_dsn(raw: Option<String>) -> Result<String> {
    let dsn = raw.context(
        "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN must be set to a direct PostgreSQL connection when KRONIKA_DEMO_WORKLOAD_DSN enables the workload",
    )?;
    anyhow::ensure!(
        !dsn.trim().is_empty(),
        "KRONIKA_DEMO_WORKLOAD_DIRECT_DSN must not be blank"
    );
    Ok(dsn)
}

impl WorkloadConfig {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let Ok(dsn) = std::env::var("KRONIKA_DEMO_WORKLOAD_DSN") else {
            return Ok(None);
        };
        let direct_dsn =
            required_direct_dsn(std::env::var("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN").ok())?;
        let config = Self {
            dsn,
            direct_dsn,
            schemas: env_u32("KRONIKA_DEMO_WORKLOAD_SCHEMAS", 1)?,
            tables_per_schema: env_u32("KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA", 8)?,
            ddl_concurrency: env_u32("KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY", 4)?,
            sessions: env_u32("KRONIKA_DEMO_WORKLOAD_SESSIONS", 4)?,
            transactions_per_second: env_u32("KRONIKA_DEMO_WORKLOAD_TPS", 20)?,
            max_orders: env_u32("KRONIKA_DEMO_WORKLOAD_MAX_ORDERS", 10_000)?,
            lock_chains: env_u32("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", 1)?,
            lock_chain_depth: env_u32("KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH", 4)?,
            lock_hold_ms: env_u64("KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS", 4_000)?,
            lock_round_interval_s: env_u64("KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S", 120)?,
            event_round_interval_s: env_u64("KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S", 180)?,
            plan_rows: env_u32("KRONIKA_DEMO_WORKLOAD_PLAN_ROWS", 300_000)?,
            plan_workers: env_u32("KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS", 4)?,
            plan_baseline_s: env_u64("KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S", 12)?,
            plan_regression_s: env_u64("KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S", 30)?,
            plan_round_interval_s: env_u64("KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S", 120)?,
            vacuum_rows: env_u32("KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS", 100_000)?,
            vacuum_round_interval_s: env_u64("KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S", 180)?,
            vacuum_statement_timeout_s: env_u64(
                "KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S",
                30,
            )?,
        };
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.dsn.trim().is_empty(),
            "KRONIKA_DEMO_WORKLOAD_DSN must not be blank"
        );
        self.validate_dimensions()?;
        self.validate_timings()
    }

    fn validate_dimensions(&self) -> Result<()> {
        for (key, value) in [
            ("KRONIKA_DEMO_WORKLOAD_SCHEMAS", self.schemas),
            (
                "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA",
                self.tables_per_schema,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY",
                self.ddl_concurrency,
            ),
            ("KRONIKA_DEMO_WORKLOAD_SESSIONS", self.sessions),
            ("KRONIKA_DEMO_WORKLOAD_TPS", self.transactions_per_second),
            ("KRONIKA_DEMO_WORKLOAD_MAX_ORDERS", self.max_orders),
            ("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", self.lock_chains),
            ("KRONIKA_DEMO_WORKLOAD_PLAN_ROWS", self.plan_rows),
            ("KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS", self.plan_workers),
            ("KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS", self.vacuum_rows),
        ] {
            anyhow::ensure!(value > 0, "{key} must be greater than zero");
        }
        for (key, value, maximum) in [
            ("KRONIKA_DEMO_WORKLOAD_SCHEMAS", self.schemas, 8),
            (
                "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA",
                self.tables_per_schema,
                64,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY",
                self.ddl_concurrency,
                16,
            ),
            ("KRONIKA_DEMO_WORKLOAD_SESSIONS", self.sessions, 16),
            (
                "KRONIKA_DEMO_WORKLOAD_TPS",
                self.transactions_per_second,
                64,
            ),
            ("KRONIKA_DEMO_WORKLOAD_MAX_ORDERS", self.max_orders, 50_000),
            ("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", self.lock_chains, 4),
            (
                "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH",
                self.lock_chain_depth,
                8,
            ),
            ("KRONIKA_DEMO_WORKLOAD_PLAN_ROWS", self.plan_rows, 500_000),
            ("KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS", self.plan_workers, 8),
            (
                "KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS",
                self.vacuum_rows,
                250_000,
            ),
        ] {
            anyhow::ensure!(value <= maximum, "{key} must be at most {maximum}");
        }
        anyhow::ensure!(
            self.tables_per_schema >= naming::COMMERCE_TABLE_COUNT,
            "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA must be at least {}",
            naming::COMMERCE_TABLE_COUNT
        );
        anyhow::ensure!(
            self.max_orders >= self.sessions,
            "KRONIKA_DEMO_WORKLOAD_MAX_ORDERS must be at least KRONIKA_DEMO_WORKLOAD_SESSIONS"
        );
        Ok(())
    }

    fn validate_timings(&self) -> Result<()> {
        anyhow::ensure!(
            self.lock_chain_depth > 1,
            "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH must be at least two"
        );
        anyhow::ensure!(
            self.lock_hold_ms > 0,
            "KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS must be greater than zero"
        );
        anyhow::ensure!(
            locks::round_has_timed_out_tail(self.lock_chain_depth, self.lock_hold_ms),
            "lock timing must let an earlier waiter acquire the row and a later waiter reach statement_timeout"
        );
        anyhow::ensure!(
            self.lock_round_interval_s > 0,
            "KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S must be greater than zero"
        );
        anyhow::ensure!(
            self.event_round_interval_s > 0,
            "KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S must be greater than zero"
        );
        anyhow::ensure!(
            self.plan_baseline_s > 0,
            "KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S must be greater than zero"
        );
        anyhow::ensure!(
            self.plan_regression_s > 0,
            "KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S must be greater than zero"
        );
        anyhow::ensure!(
            self.plan_round_interval_s > 0,
            "KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S must be greater than zero"
        );
        anyhow::ensure!(
            self.vacuum_round_interval_s > 0,
            "KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S must be greater than zero"
        );
        anyhow::ensure!(
            self.vacuum_statement_timeout_s > 0,
            "KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S must be greater than zero"
        );
        Ok(())
    }
}

fn connection_config(dsn: &str, application_name: &str) -> Result<Config> {
    let mut config = dsn
        .parse::<Config>()
        .context("parse the demo PostgreSQL workload DSN")?;
    config.application_name(application_name);
    Ok(config)
}

pub(crate) async fn connect_as(dsn: &str, application_name: &str) -> Result<Client> {
    let (client, connection) = connection_config(dsn, application_name)?
        .connect(NoTls)
        .await
        .context("connect to the demo PostgreSQL workload")?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("kronika-demo: workload connection ended: {error}");
        }
    });
    Ok(client)
}

pub(crate) struct Workload {
    runtime: Runtime,
    stop: Arc<AtomicBool>,
}

const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

impl Workload {
    pub(crate) fn start(config: WorkloadConfig) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("kronika-demo-workload")
            .build()
            .context("build the workload runtime")?;
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = Arc::clone(&stop);
        runtime.spawn(async move {
            if let Err(error) = run(config, task_stop).await {
                eprintln!("kronika-demo: workload stopped early: {error:#}");
            }
        });
        Ok(Self { runtime, stop })
    }

    pub(crate) fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.runtime.shutdown_timeout(SHUTDOWN_GRACE);
    }
}

async fn run(config: WorkloadConfig, stop: Arc<AtomicBool>) -> Result<()> {
    schema::create_all(&config).await?;

    println!(
        "kronika-demo: OLTP workload starting {} clients at up to {} transactions/s with {} reusable orders",
        config.sessions, config.transactions_per_second, config.max_orders
    );

    let mut tasks = Vec::new();
    for session in 0..config.sessions {
        let config = config.clone();
        let stop = Arc::clone(&stop);
        tasks.push(tokio::spawn(async move {
            dml::run_session(session, &config, &stop).await;
        }));
    }
    tasks.push(tokio::spawn({
        let config = config.clone();
        let stop = Arc::clone(&stop);
        async move { locks::run_rounds(&config, &stop).await }
    }));
    tasks.push(tokio::spawn({
        let config = config.clone();
        let stop = Arc::clone(&stop);
        async move { events::run_rounds(&config, &stop).await }
    }));
    tasks.push(tokio::spawn({
        let config = config.clone();
        let stop = Arc::clone(&stop);
        async move { plans::run_rounds(&config, &stop).await }
    }));
    tasks.push(tokio::spawn({
        let config = config.clone();
        let stop = Arc::clone(&stop);
        async move { vacuum::run_rounds(&config, &stop).await }
    }));
    for task in tasks {
        let _joined = task.await;
    }
    Ok(())
}

async fn wait_for_stop(stop: &AtomicBool, duration: Duration) {
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return;
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(100))).await;
    }
}

#[cfg(test)]
mod tests;
