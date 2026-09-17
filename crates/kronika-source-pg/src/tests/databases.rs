use super::{Database, ENUMERATE_QUERY, refresh};
use crate::Pool;
use std::collections::BTreeMap;

fn primary() -> Pool {
    Pool::new("host=/nonexistent dbname=postgres").expect("the DSN parses")
}

fn found(names: &[&str]) -> Vec<Database> {
    names
        .iter()
        .enumerate()
        .map(|(index, name)| Database {
            oid: u32::try_from(index).expect("a small index") + 16_400,
            name: (*name).to_owned(),
            is_current: index == 0,
        })
        .collect()
}

#[test]
fn enumeration_keeps_the_current_and_connectable_databases() {
    assert!(ENUMERATE_QUERY.contains("pg_catalog.has_database_privilege(oid, 'CONNECT')"));
    assert!(ENUMERATE_QUERY.contains("datname = pg_catalog.current_database() AS is_current"));
    assert!(ENUMERATE_QUERY.contains("WHERE datname = pg_catalog.current_database()"));
    assert!(ENUMERATE_QUERY.contains("ORDER BY datname"));
}

#[test]
fn every_non_bootstrap_database_gets_a_connection_of_its_own() {
    let mut pools = BTreeMap::new();
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    assert_eq!(pools.len(), 1);
    assert!(pools.contains_key("payments"));
}

#[test]
fn a_database_that_was_dropped_loses_its_connection() {
    let mut pools = BTreeMap::new();
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    refresh(&mut pools, &found(&["appdb"]), &primary());
    assert!(pools.is_empty());
}

#[test]
fn a_database_that_stayed_keeps_the_connection_it_had() {
    let mut pools = BTreeMap::new();
    let kept = Pool::new("host=/nonexistent dbname=sentinel").expect("the DSN parses");
    pools.insert("payments".to_owned(), kept);
    refresh(&mut pools, &found(&["appdb", "payments"]), &primary());
    let survivor = format!(
        "{:?}",
        pools.get("payments").expect("payments still has a pool")
    );
    assert!(
        survivor.contains("sentinel"),
        "the surviving pool was replaced: {survivor}"
    );
}

#[test]
fn the_bootstrap_database_does_not_get_a_duplicate_connection() {
    let mut pools = BTreeMap::new();
    refresh(&mut pools, &found(&["postgres", "appdb"]), &primary());
    assert!(!pools.contains_key("postgres"));
    assert!(pools.contains_key("appdb"));
}
