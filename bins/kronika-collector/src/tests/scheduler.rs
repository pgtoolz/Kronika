use crate::config::CollectorMode;
use std::time::{Duration, Instant};

use super::{ALL_SOURCES, DueSet, Intervals, Scheduler, SourceKind};

impl DueSet {
    pub(crate) const fn for_test(kinds: Vec<SourceKind>) -> Self {
        Self {
            kinds,
            forced: false,
        }
    }
}

fn intervals() -> Intervals {
    Intervals {
        os_core: 10,
        os_mount_topo: 60,
        os_processes: 5,
        os_process_status: 30,
        os_cgroup: 30,
        os_cgroup_mapping: 30,
        logs: 10,
        pg_activity: 10,
        pg_activity_blocked: 5,
        pg_instance: 30,
        pg_tables_and_indexes: 300,
        pg_statements_and_plans: 300,
    }
}

#[test]
fn the_first_tick_reads_every_source() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let due = scheduler.plan(Instant::now(), false);
    for kind in ALL_SOURCES {
        assert!(due.has(kind), "{kind:?} must be due on the first tick");
    }
    assert!(
        !due.forced(),
        "an initial read does not force other paced work"
    );
}

#[test]
fn a_source_comes_due_again_only_after_its_own_interval() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let start = Instant::now();
    scheduler.plan(start, false);

    let at_5s = scheduler.plan(start + Duration::from_secs(5), false);
    assert!(at_5s.has(SourceKind::OsProcesses), "5 s interval elapsed");
    assert!(!at_5s.has(SourceKind::OsCore), "10 s interval has not");
    assert!(!at_5s.has(SourceKind::OsMountTopo));

    let at_30s = scheduler.plan(start + Duration::from_secs(35), false);
    assert!(at_30s.has(SourceKind::OsCore));
    assert!(at_30s.has(SourceKind::OsProcessStatus));
    assert!(
        !at_30s.has(SourceKind::OsMountTopo),
        "60 s interval has not"
    );
}

#[test]
fn a_forced_tick_preserves_the_statement_cooldown() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let start = Instant::now();
    scheduler.plan(start, false);
    let forced = scheduler.plan(start + Duration::from_secs(1), true);
    for kind in ALL_SOURCES {
        assert_eq!(
            forced.has(kind),
            kind != SourceKind::PgStatementsAndPlans,
            "forced scheduling for {kind:?}"
        );
    }
    assert!(forced.forced());

    let without_tables = forced.without(SourceKind::PgTablesAndIndexes);
    assert!(without_tables.forced());
    let recollection = scheduler.recollection_due(&without_tables, start + Duration::from_secs(2));
    assert!(recollection.forced());
    for kind in ALL_SOURCES {
        assert_eq!(
            recollection.has(kind),
            !matches!(
                kind,
                SourceKind::PgTablesAndIndexes | SourceKind::PgStatementsAndPlans
            ),
            "recollection preserves the filtered source {kind:?}"
        );
    }
}

#[test]
fn opening_a_segment_re_reads_mount_topology() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let start = Instant::now();
    scheduler.plan(start, false);
    scheduler.mark_segment_opened();
    let next = scheduler.plan(start + Duration::from_secs(1), false);
    assert!(next.has(SourceKind::OsMountTopo), "mount and topology");
    assert!(!next.has(SourceKind::OsCore), "ordinary counters wait");
}

#[test]
fn immediate_recollection_includes_and_records_segment_open_sources() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let start = Instant::now();
    scheduler.plan(start, false);
    scheduler.mark_segment_opened();
    let original = DueSet::for_test(vec![SourceKind::OsCore]);

    let recollection = scheduler.recollection_due(&original, start + Duration::from_secs(1));

    assert!(recollection.has(SourceKind::OsCore));
    assert!(recollection.has(SourceKind::OsMountTopo));
    assert!(!recollection.has(SourceKind::OsProcesses));
    assert!(!recollection.forced());
    assert!(!recollection.without(SourceKind::OsCore).forced());
    let next = scheduler.plan(start + Duration::from_secs(2), false);
    assert!(!next.has(SourceKind::OsCore));
    assert!(!next.has(SourceKind::OsMountTopo));
}

#[test]
fn the_next_wake_is_the_soonest_positive_interval() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    let start = Instant::now();
    scheduler.plan(start, false);
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(2)),
        Some(Duration::from_secs(3)),
        "os_processes is the 5 s source"
    );
}

#[test]
fn a_zero_interval_runs_every_tick_without_pulling_the_wake_forward() {
    let mut scheduler = Scheduler::new(
        Intervals {
            os_core: 0,
            ..intervals()
        },
        CollectorMode::Local,
        true,
    );
    let start = Instant::now();
    scheduler.plan(start, false);
    let next = scheduler.plan(start, false);
    assert!(next.has(SourceKind::OsCore));
    assert_eq!(
        scheduler.next_elapsed_due_in(start),
        Some(Duration::from_secs(5)),
        "the zero-interval source is not the one that sets the wake"
    );
}

#[test]
fn postgresql_mode_never_schedules_linux_even_at_forced_segment_open() {
    let now = Instant::now();
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Postgresql, false);
    for forced in [false, true] {
        let due = scheduler.plan(now, forced);
        scheduler.mark_segment_opened();
        let due = scheduler.recollection_due(&due, now);
        assert_eq!(due.forced(), forced);
        for kind in ALL_SOURCES {
            let expected = matches!(
                kind,
                SourceKind::PgActivity
                    | SourceKind::PgInstance
                    | SourceKind::PgTablesAndIndexes
                    | SourceKind::PgStatementsAndPlans
                    | SourceKind::Logs
            );
            let expected = expected && !(forced && kind == SourceKind::PgStatementsAndPlans);
            assert_eq!(due.has(kind), expected, "configured source {kind:?}");
        }
    }
    assert_eq!(
        scheduler.next_elapsed_due_in(now),
        Some(Duration::from_secs(10))
    );
}

#[test]
fn unread_and_zero_interval_sources_do_not_request_an_earlier_wake() {
    let start = Instant::now();
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Local, true);
    assert_eq!(scheduler.next_elapsed_due_in(start), None);
    scheduler.plan(start, false);
    scheduler.mark_segment_opened();
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(2)),
        Some(Duration::from_secs(3)),
        "unread mount topology waits for the next regular wake"
    );

    let mut scheduler = Scheduler::new(
        Intervals {
            os_core: 0,
            os_mount_topo: 0,
            os_processes: 0,
            os_process_status: 0,
            os_cgroup: 0,
            os_cgroup_mapping: 0,
            logs: 0,
            pg_activity: 0,
            pg_activity_blocked: 0,
            pg_instance: 0,
            pg_tables_and_indexes: 0,
            pg_statements_and_plans: 0,
        },
        CollectorMode::Local,
        true,
    );
    scheduler.plan(start, false);
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(1)),
        Some(Duration::from_secs(299)),
        "ordinary zero intervals cannot bypass the minimum statement interval"
    );
}

#[test]
fn postgresql_groups_keep_their_independent_intervals() {
    let mut scheduler = Scheduler::new(intervals(), CollectorMode::Postgresql, false);
    let start = Instant::now();
    scheduler.plan(start, false);

    for (seconds, instance_due, tables_due) in [
        (29, false, false),
        (30, true, false),
        (299, true, false),
        (300, false, true),
    ] {
        let due = scheduler.plan(start + Duration::from_secs(seconds), false);
        assert_eq!(
            due.has(SourceKind::PgInstance),
            instance_due,
            "at {seconds}s"
        );
        assert_eq!(
            due.has(SourceKind::PgTablesAndIndexes),
            tables_due,
            "at {seconds}s"
        );
    }
}

#[test]
fn default_postgresql_groups_have_separate_cadences() {
    let mut scheduler = Scheduler::new(Intervals::default(), CollectorMode::Postgresql, false);
    let start = Instant::now();
    scheduler.plan(start, false);

    for (seconds, activity, instance, tables, statements) in [
        (9, false, false, false, false),
        (10, true, false, false, false),
        (30, true, true, false, false),
        (299, true, true, false, false),
        (300, false, false, true, true),
    ] {
        let due = scheduler.plan(start + Duration::from_secs(seconds), false);
        for (kind, expected) in [
            (SourceKind::PgActivity, activity),
            (SourceKind::PgInstance, instance),
            (SourceKind::PgTablesAndIndexes, tables),
            (SourceKind::PgStatementsAndPlans, statements),
        ] {
            assert_eq!(due.has(kind), expected, "{kind:?} at {seconds}s");
        }
    }
}

#[test]
fn forced_ticks_respect_the_configured_statement_interval_and_its_minimum() {
    for (configured, minimum_pause) in [(0, 300), (300, 300), (600, 600)] {
        let mut scheduler = Scheduler::new(
            Intervals {
                pg_statements_and_plans: configured,
                ..Intervals::default()
            },
            CollectorMode::Postgresql,
            false,
        );
        let start = Instant::now();
        assert!(
            scheduler
                .plan(start, true)
                .has(SourceKind::PgStatementsAndPlans)
        );
        assert!(
            !scheduler
                .plan(start + Duration::from_secs(minimum_pause - 1), true)
                .has(SourceKind::PgStatementsAndPlans)
        );
        assert!(
            scheduler
                .plan(start + Duration::from_secs(minimum_pause), true)
                .has(SourceKind::PgStatementsAndPlans)
        );
    }
}

#[test]
fn statement_cooldown_starts_after_postgres_finishes_and_survives_os_recollection() {
    let mut scheduler = Scheduler::new(Intervals::default(), CollectorMode::Local, true);
    let start = Instant::now();
    let due = scheduler.plan(start, false);
    scheduler.finish_postgres(&due, None, start + Duration::from_secs(45));
    scheduler.recollection_due(&due, start + Duration::from_mins(1));
    let ordinary = scheduler.plan(start + Duration::from_secs(100), false);
    assert!(!ordinary.has(SourceKind::PgStatementsAndPlans));
    scheduler.finish_postgres(&ordinary, None, start + Duration::from_mins(2));

    for seconds in [300, 344] {
        assert!(
            !scheduler
                .plan(start + Duration::from_secs(seconds), true)
                .has(SourceKind::PgStatementsAndPlans)
        );
    }
    assert!(
        scheduler
            .plan(start + Duration::from_secs(345), true)
            .has(SourceKind::PgStatementsAndPlans)
    );
}

#[test]
fn blocking_feedback_accelerates_activity_until_a_successful_clear_read() {
    let mut scheduler = Scheduler::new(
        Intervals {
            logs: 3_600,
            pg_instance: 3_600,
            pg_tables_and_indexes: 3_600,
            ..Intervals::default()
        },
        CollectorMode::Postgresql,
        false,
    );
    let start = Instant::now();
    let due = scheduler.plan(start, false);
    assert_eq!(
        scheduler.next_elapsed_due_in(start),
        Some(Duration::from_secs(10))
    );
    scheduler.finish_postgres(&due, Some(true), start + Duration::from_secs(1));
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(1)),
        Some(Duration::from_secs(4))
    );

    let due = scheduler.plan(start + Duration::from_secs(5), false);
    assert!(due.has(SourceKind::PgActivity));
    scheduler.finish_postgres(&due, None, start + Duration::from_secs(6));
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(6)),
        Some(Duration::from_secs(4))
    );

    let due = scheduler.plan(start + Duration::from_secs(10), false);
    scheduler.finish_postgres(&due, Some(false), start + Duration::from_secs(11));
    assert_eq!(
        scheduler.next_elapsed_due_in(start + Duration::from_secs(11)),
        Some(Duration::from_secs(9))
    );
}

#[test]
fn blocking_feedback_never_slows_a_faster_activity_schedule() {
    for (configured, expected_wake) in [(0, 300), (3, 3)] {
        let mut scheduler = Scheduler::new(
            Intervals {
                pg_activity: configured,
                logs: 3_600,
                pg_instance: 3_600,
                pg_tables_and_indexes: 3_600,
                ..Intervals::default()
            },
            CollectorMode::Postgresql,
            false,
        );
        let start = Instant::now();
        let due = scheduler.plan(start, false);
        scheduler.finish_postgres(&due, Some(true), start);
        assert_eq!(
            scheduler.next_elapsed_due_in(start),
            Some(Duration::from_secs(expected_wake))
        );
        assert!(
            scheduler
                .plan(start + Duration::from_secs(configured), false)
                .has(SourceKind::PgActivity)
        );
    }
}

#[test]
fn the_blocked_activity_interval_is_configurable_and_capped_by_the_base() {
    for (blocked, effective_interval, expected_wake) in [(3, 3, 3), (20, 12, 12), (0, 0, 300)] {
        let mut scheduler = Scheduler::new(
            Intervals {
                pg_activity: 12,
                pg_activity_blocked: blocked,
                logs: 3_600,
                pg_instance: 3_600,
                pg_tables_and_indexes: 3_600,
                ..Intervals::default()
            },
            CollectorMode::Postgresql,
            false,
        );
        let start = Instant::now();
        let due = scheduler.plan(start, false);
        scheduler.finish_postgres(&due, Some(true), start);
        assert_eq!(
            scheduler.next_elapsed_due_in(start),
            Some(Duration::from_secs(expected_wake)),
            "configured blocked interval {blocked}s"
        );

        let next = start + Duration::from_secs(effective_interval);
        let due = scheduler.plan(next, false);
        assert!(due.has(SourceKind::PgActivity));
        scheduler.finish_postgres(&due, Some(false), next);
        assert_eq!(
            scheduler.next_elapsed_due_in(next),
            Some(Duration::from_secs(12)),
            "a clear read restores the configured base interval"
        );
    }
}
