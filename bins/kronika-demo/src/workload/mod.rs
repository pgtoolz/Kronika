//! Optional `PostgreSQL` workload enabled by `KRONIKA_DEMO_WORKLOAD_DSN`.

pub(crate) mod config;
mod dml;
mod events;
mod locks;
mod naming;
mod plans;
mod schema;
mod vacuum;

pub(crate) use config::WorkloadConfig;

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Runtime};
use tokio_postgres::{Client, Config, NoTls};

fn connection_config(dsn: &str, application_name: &str) -> Result<Config> {
    let mut config = dsn
        .parse::<Config>()
        .map_err(|_error| anyhow::anyhow!("invalid demo PostgreSQL workload DSN"))?;
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
    runtime: Option<Runtime>,
    stop: Arc<AtomicBool>,
}

const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

impl Workload {
    pub(crate) fn start(config: WorkloadConfig) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
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
        Ok(Self {
            runtime: Some(runtime),
            stop,
        })
    }

    pub(crate) fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(SHUTDOWN_GRACE);
        }
    }
}

impl Drop for Workload {
    fn drop(&mut self) {
        self.shutdown();
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
#[path = "../tests/workload.rs"]
mod tests;
