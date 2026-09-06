use super::{WorkloadConfig, connect_as, naming};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

#[derive(Debug, Clone, Copy)]
struct Table {
    schema: u32,
    index_in_schema: u32,
}

pub(super) const CUSTOMER_ROWS: u32 = 20_000;
pub(super) const PRODUCT_ROWS: u32 = 2_048;
pub(super) const MAX_ORDER_QUANTITY: u32 = 4;

const ARCHIVE_SHAPES: [&str; 8] = [
    "(id bigint primary key, customer_id bigint not null default 0, \
     status text not null default 'paid', total_cents bigint not null default 0, \
     placed_at timestamptz not null default now())",
    "(id bigint primary key, email text not null default 'demo@example.test', \
     tier text not null default 'standard', created_at timestamptz not null default now())",
    "(id bigint primary key, order_id bigint not null default 0, product_id bigint not null default 0, \
     quantity integer not null default 1, unit_cents bigint not null default 0)",
    "(id bigint primary key, sku text not null default 'DEMO', name text not null default 'Demo product', \
     price_cents bigint not null default 0)",
    "(id bigint primary key, product_id bigint not null default 0, available integer not null default 0, \
     reserved integer not null default 0)",
    "(id bigint primary key, order_id bigint not null default 0, state text not null default 'captured', \
     amount_cents bigint not null default 0, paid_at timestamptz)",
    "(id bigint primary key, occurred_at timestamptz not null default now(), \
     kind text not null default 'checkout', payload text)",
    "(id bigint primary key, customer_id bigint not null default 0, \
     expires_at timestamptz not null default now() + interval '1 hour', \
     metadata jsonb not null default '{}'::jsonb)",
];

const PROGRESS_STEP: usize = 500;

pub(super) const fn product_price_cents(product_id: i64) -> i64 {
    500 + product_id % 50_000
}

fn table_ddl(table: Table) -> (String, String) {
    let schema = naming::schema_name(table.schema);
    let name = naming::table_name(table.schema, table.index_in_schema);
    let ddl = match table.index_in_schema {
        0 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 customer_id bigint not null references {schema}.customers(id), \
                 status text not null default 'paid' \
                     check (status in ('paid','packed','shipped','delivered','cancelled')), \
                 total_cents bigint not null check (total_cents > 0), \
                 placed_at timestamptz not null default now()\
             )"
        ),
        1 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 email text not null unique, \
                 tier text not null default 'standard' \
                     check (tier in ('standard','plus','business')), \
                 lifetime_value_cents bigint not null default 0 \
                     check (lifetime_value_cents >= 0), \
                 created_at timestamptz not null default now()\
             )"
        ),
        2 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 order_id bigint not null unique \
                     references {schema}.orders(id) on delete cascade, \
                 product_id bigint not null references {schema}.products(id), \
                 quantity integer not null check (quantity between 1 and {MAX_ORDER_QUANTITY}), \
                 unit_cents bigint not null check (unit_cents > 0)\
             ); \
             create index if not exists order_items_product_idx on {name} (product_id)"
        ),
        3 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 sku text not null unique, \
                 name text not null, \
                 price_cents bigint not null check (price_cents > 0)\
             )"
        ),
        4 => format!(
            "create table if not exists {name} (\
                 product_id bigint primary key references {schema}.products(id), \
                 available integer not null check (available >= 0), \
                 reserved integer not null default 0 check (reserved >= 0), \
                 updated_at timestamptz not null default now()\
             ) with (fillfactor = 90)"
        ),
        5 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 order_id bigint not null unique \
                     references {schema}.orders(id) on delete cascade, \
                 state text not null check (state in ('authorized','captured','refunded')), \
                 amount_cents bigint not null check (amount_cents > 0), \
                 paid_at timestamptz\
             )"
        ),
        6 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 order_id bigint references {schema}.orders(id) on delete cascade, \
                 occurred_at timestamptz not null default now(), \
                 kind text not null default 'checkout', \
                 payload text\
             ) with (fillfactor = 80); \
             create index if not exists event_log_order_idx on {name} (order_id)"
        ),
        7 => format!(
            "create table if not exists {name} (\
                 id bigint primary key, \
                 customer_id bigint not null references {schema}.customers(id), \
                 expires_at timestamptz not null default now() + interval '1 hour', \
                 metadata jsonb not null default '{{}}'::jsonb \
                     check (jsonb_typeof(metadata) = 'object')\
             ); \
             create index if not exists sessions_customer_idx on {name} (customer_id)"
        ),
        index => {
            let shape = ARCHIVE_SHAPES[index as usize % ARCHIVE_SHAPES.len()];
            format!("create table if not exists {name} {shape}")
        }
    };
    (name, ddl)
}

fn table_creation_order(tables_per_schema: u32) -> Vec<u32> {
    let mut order = vec![1, 3, 4, 0, 2, 5, 6, 7];
    order.retain(|index| *index < tables_per_schema);
    order.extend(naming::COMMERCE_TABLE_COUNT..tables_per_schema);
    order
}

fn seed_sql(max_orders: u32) -> String {
    let inventory_per_product = max_orders * MAX_ORDER_QUANTITY;
    format!(
        "begin; \
         set local statement_timeout = '60s'; \
         insert into shop.customers (id, email, tier, lifetime_value_cents, created_at) \
         select series, 'customer-' || series || '@example.test', \
                (array['standard','plus','business'])[1 + series % 3], 0, clock_timestamp() \
         from generate_series(1, {CUSTOMER_ROWS}) as series \
         on conflict (id) do nothing; \
         insert into shop.products (id, sku, name, price_cents) \
         select series, 'SKU-' || series, 'Demo product ' || series, 500 + series % 50000 \
         from generate_series(1, {PRODUCT_ROWS}) as series \
         on conflict (id) do nothing; \
         insert into shop.inventory (product_id, available, reserved, updated_at) \
         select id, {inventory_per_product}, 0, clock_timestamp() from shop.products \
         where id between 1 and {PRODUCT_ROWS} \
         on conflict (product_id) do nothing; \
         analyze shop.customers; \
         analyze shop.products; \
         analyze shop.inventory; \
         commit"
    )
}

/// Creates dependency-ordered commerce tables and their bounded reference rows.
pub(crate) async fn create_all(config: &WorkloadConfig) -> Result<()> {
    let setup = connect_as(&config.dsn, "deploy-migration")
        .await
        .context("open the schema-setup connection")?;
    for schema in 0..config.schemas {
        let name = naming::schema_name(schema);
        setup
            .batch_execute(&format!("create schema if not exists {name}"))
            .await
            .with_context(|| format!("create schema {name}"))?;
    }
    drop(setup);

    let schemas: Vec<u32> = (0..config.schemas).collect();
    let total = schemas.len() * config.tables_per_schema as usize;
    let concurrency = config.ddl_concurrency.min(config.schemas) as usize;
    let chunk_size = schemas.len().div_ceil(concurrency).max(1);
    let progress = Arc::new(AtomicUsize::new(0));
    let mut tasks = JoinSet::new();
    for chunk in schemas.chunks(chunk_size).map(<[u32]>::to_vec) {
        let dsn = config.dsn.clone();
        let progress = Arc::clone(&progress);
        let tables_per_schema = config.tables_per_schema;
        tasks.spawn(async move {
            create_chunk(&dsn, &chunk, tables_per_schema, &progress, total).await
        });
    }
    while let Some(outcome) = tasks.join_next().await {
        outcome.context("workload DDL task failed")??;
    }
    println!(
        "kronika-demo: workload schema setup created {}/{total} tables across {} schemas",
        progress.load(Ordering::Relaxed),
        config.schemas
    );

    let seed = connect_as(&config.dsn, "fixture-loader")
        .await
        .context("open the reference-data connection")?;
    seed.batch_execute(&seed_sql(config.max_orders))
        .await
        .context("seed the commerce reference data")?;
    Ok(())
}

async fn create_chunk(
    dsn: &str,
    schemas: &[u32],
    tables_per_schema: u32,
    progress: &AtomicUsize,
    total: usize,
) -> Result<()> {
    let client = connect_as(dsn, "schema-loader")
        .await
        .context("open a DDL connection")?;
    let order = table_creation_order(tables_per_schema);
    for schema in schemas {
        for index_in_schema in &order {
            let table = Table {
                schema: *schema,
                index_in_schema: *index_in_schema,
            };
            let (name, ddl) = table_ddl(table);
            client
                .batch_execute(&ddl)
                .await
                .with_context(|| format!("create table {name}"))?;
            let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if done.is_multiple_of(PROGRESS_STEP) || done == total {
                println!("kronika-demo: workload schema setup {done}/{total} tables");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
