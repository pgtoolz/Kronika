use super::{Table, seed_sql, table_creation_order, table_ddl};

#[test]
fn table_ddl_names_archive_tables_by_schema_and_index() {
    let (name, ddl) = table_ddl(Table {
        schema: 3,
        index_in_schema: 407,
    });
    assert_eq!(name, "shop_3.archive_407");
    assert!(ddl.starts_with("create table if not exists shop_3.archive_407 ("));
}

#[test]
fn commerce_tables_are_created_after_their_references() {
    assert_eq!(table_creation_order(8), [1, 3, 4, 0, 2, 5, 6, 7]);
    assert_eq!(table_creation_order(10), [1, 3, 4, 0, 2, 5, 6, 7, 8, 9]);
}

#[test]
fn commerce_schema_has_relations_constraints_and_access_indexes() {
    let statements = (0..8)
        .map(|index_in_schema| {
            table_ddl(Table {
                schema: 0,
                index_in_schema,
            })
            .1
        })
        .collect::<Vec<_>>()
        .join("; ");

    assert!(statements.contains("references shop.customers(id)"));
    assert!(statements.contains("references shop.orders(id) on delete cascade"));
    assert!(statements.contains("references shop.products(id)"));
    assert!(statements.contains("check (quantity between 1 and 4)"));
    assert!(statements.contains("check (available >= 0)"));
    assert!(statements.contains("order_items_product_idx"));
    assert!(statements.contains("event_log_order_idx"));
    assert!(statements.contains("sessions_customer_idx"));
    assert!(!statements.contains("checkout_orders_customer_placed_idx"));
}

#[test]
fn seed_rows_cover_plan_customers_and_bound_inventory_capacity() {
    let sql = seed_sql(10_000);
    assert!(sql.contains("generate_series(1, 20000)"));
    assert!(sql.contains("generate_series(1, 2048)"));
    assert!(sql.contains("select id, 40000, 0"));
    assert!(sql.contains("on conflict (product_id) do nothing"));
    assert!(sql.starts_with("begin;"));
    assert!(sql.ends_with("commit"));
}
