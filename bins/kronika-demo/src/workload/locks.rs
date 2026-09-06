//! `PostgreSQL` queues each chain's conflicting row locks in FIFO order.

use super::{WorkloadConfig, connect_as, naming, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(test)]
mod tests;

const LOCK_STATEMENT_TIMEOUT_S: u64 = 10;

pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let table = naming::table_name(0, 0);
    if let Err(error) = ensure_keys(&config.dsn, &table, config.lock_chains).await {
        eprintln!("kronika-demo: could not seed lock-chain keys: {error:#}");
        return;
    }

    // Offset lock episodes from plan-regression episodes.
    wait_for_stop(stop, Duration::from_secs(65)).await;

    while !stop.load(Ordering::Relaxed) {
        println!("kronika-demo: lock story starting checkout waiters");
        run_one_round(config, &table).await;
        wait_for_stop(stop, Duration::from_secs(config.lock_round_interval_s)).await;
    }
}

async fn ensure_keys(dsn: &str, table: &str, chains: u32) -> anyhow::Result<()> {
    let client = connect_as(dsn, "scenario-setup").await?;
    for key in 0..chains {
        client
            .batch_execute(&format!(
                "insert into {table} (id, customer_id, status, total_cents, placed_at) \
                 values ({key}, 1, 'paid', 1000, clock_timestamp()) on conflict do nothing"
            ))
            .await?;
    }
    Ok(())
}

async fn run_one_round(config: &WorkloadConfig, table: &str) {
    let mut chains = Vec::new();
    for chain in periodic_chain_keys(config.lock_chains) {
        let config = config.clone();
        let table = table.to_owned();
        chains.push(tokio::spawn(async move {
            run_chain(&config, &table, i64::from(chain)).await;
        }));
    }
    for chain in chains {
        let _joined = chain.await;
    }
}

async fn run_chain(config: &WorkloadConfig, table: &str, key: i64) {
    let root = match connect_as(&config.dsn, link_application_name(0)).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: root lock holder could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = root.batch_execute("begin").await {
        eprintln!("kronika-demo: root lock holder could not begin: {error:?}");
        return;
    }
    if let Err(error) = root.batch_execute(&lock_update_sql(table, key)).await {
        eprintln!("kronika-demo: root lock holder could not lock row {key}: {error:?}");
        drop(root.batch_execute("rollback").await);
        return;
    }

    let hold = Duration::from_millis(config.lock_hold_ms);
    let mut waiters = Vec::new();
    for link in 1..config.lock_chain_depth {
        let dsn = config.dsn.clone();
        let table = table.to_owned();
        waiters.push(tokio::spawn(async move {
            hold_one_link(&dsn, link_application_name(link), &table, key, hold).await;
        }));
    }

    tokio::time::sleep(hold).await;
    if let Err(error) = root.batch_execute("commit").await {
        eprintln!("kronika-demo: root lock holder could not commit: {error:?}");
    }
    for waiter in waiters {
        let _joined = waiter.await;
    }
}

const fn periodic_chain_keys(chains: u32) -> std::ops::Range<u32> {
    0..chains
}

pub(super) fn round_has_timed_out_tail(depth: u32, hold_ms: u64) -> bool {
    let timeout_ms = u128::from(LOCK_STATEMENT_TIMEOUT_S) * 1_000;
    let hold_ms = u128::from(hold_ms);
    let longest_wait_ms = hold_ms * u128::from(depth.saturating_sub(1));
    hold_ms < timeout_ms && longest_wait_ms > timeout_ms
}

const fn link_application_name(link: u32) -> &'static str {
    if link == 0 {
        "payment-reconciler"
    } else {
        "checkout-api"
    }
}

fn lock_update_sql(table: &str, key: i64) -> String {
    format!(
        "set local statement_timeout = '{LOCK_STATEMENT_TIMEOUT_S}s'; \
         update {table} \
         set status = case status when 'paid' then 'packed' else 'paid' end \
         where id = {key}"
    )
}

// The simple-query protocol avoids prepared statements across pooled backends.
async fn hold_one_link(dsn: &str, application_name: &str, table: &str, key: i64, hold: Duration) {
    let client = match connect_as(dsn, application_name).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: lock-chain link could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = client.batch_execute("begin").await {
        eprintln!("kronika-demo: lock-chain link could not begin: {error:?}");
        return;
    }
    let locked = client.batch_execute(&lock_update_sql(table, key)).await;
    if let Err(error) = locked {
        eprintln!("kronika-demo: lock-chain link could not lock row {key}: {error:?}");
        drop(client.batch_execute("rollback").await);
        return;
    }
    tokio::time::sleep(hold).await;
    if let Err(error) = client.batch_execute("commit").await {
        eprintln!("kronika-demo: lock-chain link could not commit: {error:?}");
    }
}
