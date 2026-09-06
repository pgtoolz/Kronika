use super::{WorkloadConfig, connect_as, schema, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Purchase {
    order_id: i64,
    customer_id: i64,
    product_id: i64,
    quantity: i32,
    unit_cents: i64,
    total_cents: i64,
    session_id: i64,
}

pub(crate) fn session_application_name(session: u32) -> String {
    format!("shop-oltp-{:02}", session + 1)
}

fn transaction_period(sessions: u32, transactions_per_second: u32) -> Duration {
    let nanos = NANOS_PER_SECOND * u64::from(sessions);
    Duration::from_nanos(nanos.div_ceil(u64::from(transactions_per_second)))
}

fn session_start_offset(period: Duration, session: u32, sessions: u32) -> Duration {
    let nanos = period.as_nanos() * u128::from(session) / u128::from(sessions);
    Duration::from_nanos(u64::try_from(nanos).expect("validated workload period must fit in u64"))
}

fn session_slot(session: u32, sequence: u64, sessions: u32, max_orders: u32) -> u32 {
    let common = max_orders / sessions;
    let extra = max_orders % sessions;
    let length = common + u32::from(session < extra);
    let start = session * common + session.min(extra);
    let offset = u32::try_from(sequence % u64::from(length))
        .expect("the remainder of a u32 slot count must fit in u32");
    start + offset
}

fn purchase(session: u32, sequence: u64, config: &WorkloadConfig) -> Purchase {
    let slot = session_slot(session, sequence, config.sessions, config.max_orders);
    let row_base = i64::from(config.plan_rows.max(config.vacuum_rows)) + 1;
    let order_id = row_base + i64::from(slot);
    let product_id = i64::from(slot % schema::PRODUCT_ROWS + 1);
    let customer_offset = sequence
        .wrapping_mul(7_919)
        .wrapping_add(u64::from(session) * 104_729)
        % u64::from(schema::CUSTOMER_ROWS);
    let customer_id =
        i64::try_from(customer_offset + 1).expect("the bounded customer ID must fit in i64");
    let quantity_u32 = u32::try_from(
        sequence.wrapping_add(u64::from(session)) % u64::from(schema::MAX_ORDER_QUANTITY) + 1,
    )
    .expect("the bounded quantity must fit in u32");
    let quantity = i32::try_from(quantity_u32).expect("the bounded quantity must fit in i32");
    let unit_cents = schema::product_price_cents(product_id);
    Purchase {
        order_id,
        customer_id,
        product_id,
        quantity,
        unit_cents,
        total_cents: unit_cents * i64::from(quantity),
        session_id: i64::from(session) + 1,
    }
}

pub(crate) async fn run_session(session: u32, config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let application_name = session_application_name(session);
    let Ok(client) = connect_as(&config.dsn, &application_name).await else {
        eprintln!("kronika-demo: OLTP client {session} could not connect");
        return;
    };
    let period = transaction_period(config.sessions, config.transactions_per_second);
    wait_for_stop(stop, session_start_offset(period, session, config.sessions)).await;

    let mut sequence = 0_u64;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let purchase = purchase(session, sequence, config);
        if let Err(error) = client.batch_execute(&transaction_sql(purchase)).await {
            let error = anyhow::Error::new(error);
            eprintln!(
                "kronika-demo: OLTP client {session} order {} failed: {error:#}",
                purchase.order_id
            );
            if let Err(rollback_error) = client.batch_execute("rollback").await {
                eprintln!(
                    "kronika-demo: OLTP client {session} rollback failed: {rollback_error:#}"
                );
            }
        }
        sequence = sequence.wrapping_add(1);
        wait_for_stop(stop, period.saturating_sub(started.elapsed())).await;
    }
}

// One Simple Query message keeps the transaction on one pooled backend without
// relying on prepared statements that survive a PgBouncer transaction boundary.
fn transaction_sql(purchase: Purchase) -> String {
    let Purchase {
        order_id,
        customer_id,
        product_id,
        quantity,
        unit_cents,
        total_cents,
        session_id,
    } = purchase;
    format!(
        "begin; \
         set local statement_timeout = '3s'; \
         set local lock_timeout = '1s'; \
         select tier, lifetime_value_cents from shop.customers where id = {customer_id}; \
         select products.price_cents, inventory.available, inventory.reserved \
         from shop.products as products \
         join shop.inventory as inventory on inventory.product_id = products.id \
         where products.id = {product_id} for update of inventory; \
         update shop.inventory as inventory \
         set available = inventory.available + old_item.quantity, \
             reserved = inventory.reserved - old_item.quantity, \
             updated_at = clock_timestamp() \
         from shop.order_items as old_item \
         where old_item.order_id = {order_id} \
           and old_item.product_id = inventory.product_id; \
         delete from shop.orders where id = {order_id}; \
         update shop.inventory \
         set available = available - {quantity}, \
             reserved = reserved + {quantity}, \
             updated_at = clock_timestamp() \
         where product_id = {product_id} and available >= {quantity}; \
         update shop.customers \
         set lifetime_value_cents = lifetime_value_cents + {total_cents} \
         where id = {customer_id}; \
         insert into shop.orders (id, customer_id, status, total_cents, placed_at) \
         values ({order_id}, {customer_id}, 'paid', {total_cents}, clock_timestamp()); \
         insert into shop.order_items \
             (id, order_id, product_id, quantity, unit_cents) \
         values ({order_id}, {order_id}, {product_id}, {quantity}, {unit_cents}); \
         insert into shop.payments (id, order_id, state, amount_cents, paid_at) \
         values ({order_id}, {order_id}, 'captured', {total_cents}, clock_timestamp()); \
         insert into shop.event_log (id, order_id, occurred_at, kind, payload) \
         values ({order_id}, {order_id}, clock_timestamp(), 'order-paid', \
                 'payment captured by demo OLTP'); \
         insert into shop.sessions (id, customer_id, expires_at, metadata) \
         values ({session_id}, {customer_id}, clock_timestamp() + interval '15 minutes', \
                 '{{\"source\":\"oltp\"}}'::jsonb) \
         on conflict (id) do update \
         set customer_id = excluded.customer_id, \
             expires_at = excluded.expires_at, \
             metadata = excluded.metadata; \
         commit"
    )
}

#[cfg(test)]
mod tests;
