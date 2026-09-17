use super::{FinderSurface, cadence_lookback};

#[test]
fn policies_keep_legacy_defaults_and_select_recorded_family_cadences() {
    let cases = [
        (FinderSurface::Processes, "os_process", None, 5),
        (
            FinderSurface::Tables,
            "pg_stat_user_tables",
            Some("postgresql_relations_interval_seconds"),
            300,
        ),
        (
            FinderSurface::Indexes,
            "pg_stat_user_indexes",
            Some("postgresql_relations_interval_seconds"),
            300,
        ),
        (
            FinderSurface::Activity,
            "pg_stat_activity",
            Some("postgresql_interval_seconds"),
            30,
        ),
        (
            FinderSurface::Locks,
            "pg_locks",
            Some("postgresql_interval_seconds"),
            30,
        ),
        (
            FinderSurface::Vacuum,
            "pg_stat_progress_vacuum",
            Some("postgresql_interval_seconds"),
            30,
        ),
        (
            FinderSurface::Databases,
            "pg_stat_database",
            Some("postgresql_instance_interval_seconds"),
            30,
        ),
        (
            FinderSurface::Statements,
            "pg_stat_statements",
            Some("postgresql_statements_interval_seconds"),
            30,
        ),
        (
            FinderSurface::Plans,
            "pg_store_plans",
            Some("postgresql_statements_interval_seconds"),
            30,
        ),
    ];

    for (surface, logical_name, cadence_column, default_cadence) in cases {
        let policy = surface.policy();
        assert_eq!(policy.logical_name, logical_name);
        assert_eq!(policy.cadence_column, cadence_column);
        assert_eq!(policy.default_cadence_seconds, default_cadence);
    }
}

#[test]
fn lookback_has_a_twenty_second_floor_and_checked_arithmetic() {
    assert_eq!(cadence_lookback(0).expect("zero cadence"), 20_000_000);
    assert_eq!(cadence_lookback(8).expect("eight seconds"), 20_000_000);
    assert_eq!(cadence_lookback(9).expect("nine seconds"), 22_500_000);
    assert!(cadence_lookback(u64::MAX).is_err());
}
