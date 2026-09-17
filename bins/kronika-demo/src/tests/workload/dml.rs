use super::{
    Purchase, purchase, session_application_name, session_slot, session_start_offset,
    transaction_period, transaction_sql,
};
use crate::workload::tests::config;
use std::time::Duration;

#[test]
fn clients_have_stable_visible_oltp_names() {
    assert_eq!(session_application_name(0), "shop-oltp-01");
    assert_eq!(session_application_name(3), "shop-oltp-04");
    assert_eq!(session_application_name(15), "shop-oltp-16");
}

#[test]
fn aggregate_rate_is_never_faster_than_the_configured_tps() {
    assert_eq!(transaction_period(4, 20), Duration::from_millis(200));
    assert_eq!(transaction_period(4, 24).as_nanos(), 166_666_667);
    assert_eq!(
        session_start_offset(Duration::from_millis(200), 0, 4),
        Duration::ZERO
    );
    assert_eq!(
        session_start_offset(Duration::from_millis(200), 3, 4),
        Duration::from_millis(150)
    );
}

#[test]
fn client_slot_ranges_are_disjoint_and_cover_the_order_cap() {
    let mut slots = Vec::new();
    for session in 0..4 {
        for sequence in 0..3 {
            slots.push(session_slot(session, sequence, 4, 10));
        }
    }
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots, (0..10).collect::<Vec<_>>());
    assert_eq!(session_slot(0, 3, 4, 10), 0);
    assert_eq!(session_slot(3, 2, 4, 10), 8);
}

#[test]
fn purchases_stay_above_showcase_ids_and_inside_reference_data() {
    let config = config();
    let first = purchase(0, 0, &config);
    let last = purchase(3, 9_999, &config);
    let first_oltp_id = i64::from(config.plan_rows.max(config.vacuum_rows)) + 1;
    for value in [first, last] {
        assert!(value.order_id >= first_oltp_id);
        assert!(value.order_id < first_oltp_id + i64::from(config.max_orders));
        assert!((1..=20_000).contains(&value.customer_id));
        assert!((1..=2_048).contains(&value.product_id));
        assert!((1..=4).contains(&value.quantity));
        assert_eq!(
            value.total_cents,
            value.unit_cents * i64::from(value.quantity)
        );
    }
}

#[test]
fn one_transaction_reads_indexed_entities_and_writes_a_complete_order() {
    let sql = transaction_sql(Purchase {
        order_id: 300_001,
        customer_id: 42,
        product_id: 7,
        quantity: 3,
        unit_cents: 507,
        total_cents: 1_521,
        session_id: 2,
    });
    assert!(sql.starts_with("begin;"));
    assert!(sql.ends_with("commit"));
    assert!(sql.contains("from shop.customers where id = 42"));
    assert!(sql.contains("join shop.inventory as inventory"));
    assert!(sql.contains("where products.id = 7 for update of inventory"));
    assert!(sql.contains("update shop.inventory"));
    assert!(sql.contains("delete from shop.orders where id = 300001"));
    assert!(sql.contains("insert into shop.orders"));
    assert!(sql.contains("insert into shop.order_items"));
    assert!(sql.contains("insert into shop.payments"));
    assert!(sql.contains("insert into shop.event_log"));
    assert!(sql.contains("insert into shop.sessions"));
}

#[test]
fn steady_oltp_has_no_old_placeholder_or_collector_queries() {
    let sql = transaction_sql(Purchase {
        order_id: 300_001,
        customer_id: 42,
        product_id: 7,
        quantity: 3,
        unit_cents: 507,
        total_cents: 1_521,
        session_id: 2,
    });
    for rejected in ["set id = id", "select *", "where false", "/* kronika:"] {
        assert!(!sql.contains(rejected), "steady OLTP contains {rejected:?}");
    }
}
