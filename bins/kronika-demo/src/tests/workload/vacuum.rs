use super::{run_sql, setup_sql};

#[test]
fn setup_creates_a_visible_table_of_the_requested_size() {
    let sql = setup_sql(100_000, 30);
    assert!(sql.starts_with("set statement_timeout = '30s'"));
    assert!(sql.contains("shop.event_log"));
    assert!(sql.contains("generate_series(1, 100000)"));
    assert!(sql.contains("repeat(md5(series::text), 8)"));
    assert!(sql.contains("where not exists (select 1 from shop.event_log where id = 100000)"));
}

#[test]
fn each_round_has_a_bounded_update_and_throttled_vacuum() {
    let sql = run_sql(30).join("; ");
    assert!(sql.contains("statement_timeout = '30s'"));
    assert!(!sql.contains("statement_timeout = 0"));
    assert!(sql.contains("update shop.event_log"));
    assert!(sql.contains("vacuum (analyze) shop.event_log"));
    assert!(sql.contains("vacuum_cost_delay = '8ms'"));
    assert!(sql.contains("vacuum_cost_limit = 200"));
}
