use super::{
    link_application_name, lock_update_sql, periodic_chain_keys, round_has_timed_out_tail,
};

#[test]
fn all_chains_run_in_periodic_rounds() {
    assert_eq!(periodic_chain_keys(1).collect::<Vec<_>>(), [0]);
    assert_eq!(periodic_chain_keys(2).collect::<Vec<_>>(), [0, 1]);
}

#[test]
fn the_default_round_times_out_only_after_an_earlier_waiter_can_run() {
    assert!(round_has_timed_out_tail(4, 4_000));
    assert!(!round_has_timed_out_tail(3, 4_000));
    assert!(!round_has_timed_out_tail(4, 10_000));
}

#[test]
fn every_lock_update_sets_a_finite_statement_timeout() {
    assert_eq!(
        lock_update_sql("shop.orders", 7),
        "set local statement_timeout = '10s'; update shop.orders set status = case status when 'paid' then 'packed' else 'paid' end where id = 7"
    );
}

#[test]
fn the_holder_and_checkout_waiters_have_distinct_visible_roles() {
    assert_eq!(link_application_name(0), "payment-reconciler");
    assert_eq!(link_application_name(1), "checkout-api");
    assert_eq!(link_application_name(99), "checkout-api");
}
