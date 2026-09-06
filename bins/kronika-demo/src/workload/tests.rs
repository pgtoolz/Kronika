use super::{WorkloadConfig, connection_config, required_direct_dsn};

pub(super) fn config() -> WorkloadConfig {
    WorkloadConfig {
        dsn: "host=127.0.0.1 password=private".to_owned(),
        direct_dsn: "host=/var/run/postgresql password=also-private".to_owned(),
        schemas: 4,
        tables_per_schema: 40,
        ddl_concurrency: 4,
        sessions: 4,
        transactions_per_second: 20,
        max_orders: 10_000,
        lock_chains: 1,
        lock_chain_depth: 4,
        lock_hold_ms: 4_000,
        lock_round_interval_s: 45,
        event_round_interval_s: 60,
        plan_rows: 300_000,
        plan_workers: 4,
        plan_baseline_s: 12,
        plan_regression_s: 30,
        plan_round_interval_s: 120,
        vacuum_rows: 100_000,
        vacuum_round_interval_s: 180,
        vacuum_statement_timeout_s: 30,
    }
}

#[test]
fn debug_output_redacts_the_workload_dsn() {
    let output = format!("{:?}", config());
    assert!(output.contains("[redacted]"));
    assert!(!output.contains("private"));
    assert!(!output.contains("also-private"));
    assert!(!output.contains("127.0.0.1"));
}

#[test]
fn direct_only_scenarios_require_an_explicit_postgresql_connection() {
    assert!(required_direct_dsn(None).is_err());
    assert!(required_direct_dsn(Some("   ".to_owned())).is_err());
    assert_eq!(
        required_direct_dsn(Some("host=postgres port=5432".to_owned())).unwrap(),
        "host=postgres port=5432"
    );
}

#[test]
fn scenario_connections_override_the_generic_dsn_identity() {
    let config = connection_config(
        "host=127.0.0.1 application_name=generic-workload",
        "checkout-api",
    )
    .unwrap();
    assert_eq!(config.get_application_name(), Some("checkout-api"));
}

#[test]
fn workload_dimensions_and_timers_must_be_positive() {
    let valid = config();
    assert!(valid.validate().is_ok());

    for invalidate in [
        |value: &mut WorkloadConfig| value.schemas = 0,
        |value: &mut WorkloadConfig| value.tables_per_schema = 0,
        |value: &mut WorkloadConfig| value.ddl_concurrency = 0,
        |value: &mut WorkloadConfig| value.sessions = 0,
        |value: &mut WorkloadConfig| value.transactions_per_second = 0,
        |value: &mut WorkloadConfig| value.max_orders = 0,
        |value: &mut WorkloadConfig| value.lock_chains = 0,
        |value: &mut WorkloadConfig| value.plan_rows = 0,
        |value: &mut WorkloadConfig| value.plan_workers = 0,
        |value: &mut WorkloadConfig| value.vacuum_rows = 0,
    ] {
        let mut invalid = valid.clone();
        invalidate(&mut invalid);
        assert!(invalid.validate().is_err());
    }

    for depth in [0, 1] {
        let mut invalid = valid.clone();
        invalid.lock_chain_depth = depth;
        assert!(invalid.validate().is_err());
    }

    let mut no_timed_out_tail = valid.clone();
    no_timed_out_tail.lock_chain_depth = 3;
    assert!(no_timed_out_tail.validate().is_err());

    let mut invalid = valid.clone();
    invalid.lock_hold_ms = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = valid.clone();
    invalid.lock_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = valid;
    invalid.event_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    for invalidate in [
        |value: &mut WorkloadConfig| value.plan_baseline_s = 0,
        |value: &mut WorkloadConfig| value.plan_regression_s = 0,
        |value: &mut WorkloadConfig| value.plan_round_interval_s = 0,
    ] {
        let mut invalid = config();
        invalidate(&mut invalid);
        assert!(invalid.validate().is_err());
    }

    let mut invalid = config();
    invalid.vacuum_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = config();
    invalid.vacuum_statement_timeout_s = 0;
    assert!(invalid.validate().is_err());
}

#[test]
fn workload_connections_and_data_have_hard_upper_bounds() {
    for invalidate in [
        |value: &mut WorkloadConfig| value.schemas = 9,
        |value: &mut WorkloadConfig| value.tables_per_schema = 65,
        |value: &mut WorkloadConfig| value.ddl_concurrency = 17,
        |value: &mut WorkloadConfig| value.sessions = 17,
        |value: &mut WorkloadConfig| value.transactions_per_second = 65,
        |value: &mut WorkloadConfig| value.max_orders = 50_001,
        |value: &mut WorkloadConfig| value.lock_chains = 5,
        |value: &mut WorkloadConfig| value.lock_chain_depth = 9,
        |value: &mut WorkloadConfig| value.plan_rows = 500_001,
        |value: &mut WorkloadConfig| value.plan_workers = 9,
        |value: &mut WorkloadConfig| value.vacuum_rows = 250_001,
    ] {
        let mut invalid = config();
        invalidate(&mut invalid);
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn commerce_schema_and_order_ring_fit_every_oltp_client() {
    let mut invalid = config();
    invalid.tables_per_schema = 7;
    assert!(invalid.validate().is_err());

    let mut invalid = config();
    invalid.sessions = 5;
    invalid.max_orders = 4;
    assert!(invalid.validate().is_err());
}
