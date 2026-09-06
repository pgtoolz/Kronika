//! Restores the demo index before starting another plan-regression round.

use super::{WorkloadConfig, connect_as, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::task::JoinSet;

const TABLE: &str = "shop.orders";
const INDEX: &str = "checkout_orders_customer_placed_idx";

struct TransitionSql {
    drop_index: String,
    restore_index: String,
}

fn setup_sql(rows: u32) -> String {
    format!(
        "set statement_timeout = '60s'; \
         insert into {TABLE} (id, customer_id, status, total_cents, placed_at) \
         select series, 1 + (series - 1) % 20000, \
                (array['paid','packed','shipped','delivered'])[1 + series % 4], \
                1000 + series % 50000, \
                clock_timestamp() - ({rows} - series) * interval '1 second' \
         from generate_series(1, {rows}) as series \
         where not exists (select 1 from {TABLE} where id = {rows}) \
         on conflict (id) do nothing; \
         create index if not exists {INDEX} on {TABLE} (customer_id, placed_at desc); \
         analyze {TABLE}"
    )
}

fn checkout_query_sql(customer_id: u32) -> String {
    format!(
        "select id, status, total_cents from {TABLE} \
         where customer_id = {customer_id} order by placed_at desc limit 50"
    )
}

fn transition_sql() -> TransitionSql {
    TransitionSql {
        drop_index: format!(
            "set lock_timeout = '3s'; set statement_timeout = '10s'; \
             drop index if exists shop.{INDEX}"
        ),
        restore_index: format!(
            "set lock_timeout = '3s'; set statement_timeout = '10s'; \
             create index if not exists {INDEX} on {TABLE} (customer_id, placed_at desc); \
             analyze {TABLE}"
        ),
    }
}

async fn restore_index(dsn: &str, sql: &str) {
    loop {
        let recovery = async {
            let client = connect_as(dsn, "deploy-recovery").await?;
            client.batch_execute(sql).await?;
            anyhow::Ok(())
        }
        .await;
        match recovery {
            Ok(()) => return,
            Err(error) => {
                eprintln!("kronika-demo: plan index recovery failed, retrying: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let ddl = match connect_as(&config.direct_dsn, "deploy-migration").await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: plan story could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = ddl.batch_execute(&setup_sql(config.plan_rows)).await {
        eprintln!("kronika-demo: plan story setup failed: {error:#}");
        return;
    }

    while !stop.load(Ordering::Relaxed) {
        println!("kronika-demo: plan story entering indexed checkout baseline");
        run_checkout_window(config, stop, Duration::from_secs(config.plan_baseline_s)).await;
        if stop.load(Ordering::Relaxed) {
            return;
        }

        let transition = transition_sql();
        if let Err(error) = ddl.batch_execute(&transition.drop_index).await {
            eprintln!("kronika-demo: plan story could not remove its index: {error:#}");
            wait_for_stop(stop, Duration::from_secs(config.plan_round_interval_s)).await;
            continue;
        }

        println!("kronika-demo: plan story entering sequential-scan regression");
        run_checkout_window(config, stop, Duration::from_secs(config.plan_regression_s)).await;

        restore_index(&config.direct_dsn, &transition.restore_index).await;
        println!("kronika-demo: plan story restored the checkout index");
        if stop.load(Ordering::Relaxed) {
            return;
        }

        run_checkout_window(config, stop, Duration::from_secs(config.plan_baseline_s)).await;
        wait_for_stop(stop, Duration::from_secs(config.plan_round_interval_s)).await;
    }
}

async fn run_checkout_window(config: &WorkloadConfig, stop: &Arc<AtomicBool>, duration: Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    let mut workers = JoinSet::new();
    for worker in 0..config.plan_workers {
        let dsn = config.direct_dsn.clone();
        let stop = Arc::clone(stop);
        workers.spawn(async move {
            let client = match connect_as(&dsn, "checkout-api").await {
                Ok(client) => client,
                Err(error) => {
                    eprintln!(
                        "kronika-demo: checkout worker {worker} could not connect: {error:#}"
                    );
                    return;
                }
            };
            if let Err(error) = client.batch_execute("set statement_timeout = '3s'").await {
                eprintln!(
                    "kronika-demo: checkout worker {worker} could not set its timeout: {error:#}"
                );
                return;
            }
            let query = checkout_query_sql(4_242 + worker);
            while !stop.load(Ordering::Relaxed) && tokio::time::Instant::now() < deadline {
                if let Err(error) = client.batch_execute(&query).await {
                    eprintln!("kronika-demo: checkout worker {worker} failed: {error:#}");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }
    while workers.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests;
