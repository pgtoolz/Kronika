use super::{commerce_table_names, schema_name, table_name};

#[test]
fn schema_name_is_a_stable_shop_prefix() {
    assert_eq!(schema_name(0), "shop");
    assert_eq!(schema_name(12), "shop_12");
}

#[test]
fn the_primary_schema_uses_business_table_names() {
    assert_eq!(
        commerce_table_names(),
        [
            "orders",
            "customers",
            "order_items",
            "products",
            "inventory",
            "payments",
            "event_log",
            "sessions",
        ]
    );
    assert_eq!(table_name(0, 0), "shop.orders");
    assert_eq!(table_name(0, 7), "shop.sessions");
    assert_eq!(table_name(0, 8), "shop.archive_8");
    assert_eq!(table_name(3, 7), "shop_3.sessions");
}
