use super::{FinderSurface, cadence_lookback};

#[test]
fn policies_keep_the_nine_surface_cadences() {
    let cases = [
        (FinderSurface::Processes, "os_process", Some(5), 20_000_000),
        (
            FinderSurface::Tables,
            "pg_stat_user_tables",
            Some(300),
            750_000_000,
        ),
        (
            FinderSurface::Indexes,
            "pg_stat_user_indexes",
            Some(300),
            750_000_000,
        ),
        (
            FinderSurface::Activity,
            "pg_stat_activity",
            None,
            75_000_000,
        ),
        (FinderSurface::Locks, "pg_locks", None, 75_000_000),
        (
            FinderSurface::Vacuum,
            "pg_stat_progress_vacuum",
            None,
            75_000_000,
        ),
        (
            FinderSurface::Databases,
            "pg_stat_database",
            None,
            75_000_000,
        ),
        (
            FinderSurface::Statements,
            "pg_stat_statements",
            None,
            75_000_000,
        ),
        (FinderSurface::Plans, "pg_store_plans", None, 75_000_000),
    ];

    for (surface, logical_name, cadence, lookback) in cases {
        let policy = surface.policy();
        assert_eq!(policy.logical_name, logical_name);
        assert_eq!(policy.fixed_cadence_seconds, cadence);
        assert_eq!(
            cadence_lookback(cadence.unwrap_or(30)).expect("bounded cadence"),
            lookback
        );
    }
}

#[test]
fn lookback_has_a_twenty_second_floor_and_checked_arithmetic() {
    assert_eq!(cadence_lookback(0).expect("zero cadence"), 20_000_000);
    assert_eq!(cadence_lookback(8).expect("eight seconds"), 20_000_000);
    assert_eq!(cadence_lookback(9).expect("nine seconds"), 22_500_000);
    assert!(cadence_lookback(u64::MAX).is_err());
}
