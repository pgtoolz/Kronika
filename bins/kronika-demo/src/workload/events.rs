use super::{WorkloadConfig, connect_as, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_postgres::Client;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// A query that deliberately crosses the 5s known-bad boundary.
    SlowQuery,
    /// Emits a syntax error.
    BadStatement,
    /// Emits `PgBouncer`'s "no such database" event.
    BadDatabase,
}

pub(crate) const fn episode_actions() -> [Action; 3] {
    [Action::SlowQuery, Action::BadStatement, Action::BadDatabase]
}

pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let client = match connect_as(&config.dsn, "catalog-api").await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: event workload could not connect: {error:#}");
            return;
        }
    };
    wait_for_stop(stop, Duration::from_secs(140)).await;
    while !stop.load(Ordering::Relaxed) {
        for action in episode_actions() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if let Err(error) = perform(&client, config, action).await {
                eprintln!("kronika-demo: event action {action:?} failed: {error:#}");
            }
        }
        wait_for_stop(stop, Duration::from_secs(config.event_round_interval_s)).await;
    }
}

async fn perform(client: &Client, config: &WorkloadConfig, action: Action) -> anyhow::Result<()> {
    match action {
        Action::SlowQuery => client.batch_execute("select pg_sleep(6)").await?,
        Action::BadStatement => {
            // The failure is the event.
            drop(client.batch_execute("slect 1").await);
        }
        Action::BadDatabase => {
            drop(connect_as(&format!("{} dbname=nope", config.dsn), "misconfigured-api").await);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
