use std::time::{Duration, Instant};

use super::{Intervals, Scheduler, SourceKind};

fn intervals() -> Intervals {
    Intervals {
        os_core: 10,
        os_mount_topo: 60,
        os_processes: 5,
        os_process_status: 30,
        os_cgroup: 30,
        os_cgroup_mapping: 30,
        logs: 10,
        pg: 30,
        pg_relations: 300,
    }
}

#[test]
fn the_first_tick_reads_every_source() {
    let mut scheduler = Scheduler::new(intervals());
    let due = scheduler.plan(Instant::now(), false);
    assert!(due.has(SourceKind::OsCore));
    assert!(due.has(SourceKind::OsMountTopo));
}

#[test]
fn a_source_comes_due_again_only_after_its_own_interval() {
    let mut scheduler = Scheduler::new(intervals());
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
fn a_forced_tick_reads_everything_regardless_of_intervals() {
    let mut scheduler = Scheduler::new(intervals());
    let start = Instant::now();
    scheduler.plan(start, false);
    let forced = scheduler.plan(start + Duration::from_secs(1), true);
    for kind in super::ALL_SOURCES {
        assert!(forced.has(kind), "{kind:?} must be due on a forced tick");
    }
    assert!(forced.forced());
}

#[test]
fn opening_a_segment_re_reads_mount_topology() {
    let mut scheduler = Scheduler::new(intervals());
    let start = Instant::now();
    scheduler.plan(start, false);
    scheduler.mark_segment_opened();
    let next = scheduler.plan(start + Duration::from_secs(1), false);
    assert!(next.has(SourceKind::OsMountTopo), "mount and topology");
    assert!(!next.has(SourceKind::OsCore), "ordinary counters wait");
}

#[test]
fn immediate_recollection_includes_and_records_segment_open_sources() {
    let mut scheduler = Scheduler::new(intervals());
    let start = Instant::now();
    scheduler.plan(start, false);
    scheduler.mark_segment_opened();
    let original = super::DueSet::for_test(vec![SourceKind::OsCore]);

    let recollection = scheduler.recollection_due(&original, start + Duration::from_secs(1));

    assert!(recollection.has(SourceKind::OsCore));
    assert!(recollection.has(SourceKind::OsMountTopo));
    assert!(!recollection.has(SourceKind::OsProcesses));
    let next = scheduler.plan(start + Duration::from_secs(2), false);
    assert!(!next.has(SourceKind::OsCore));
    assert!(!next.has(SourceKind::OsMountTopo));
}

#[test]
fn the_next_wake_is_the_soonest_positive_interval() {
    let mut scheduler = Scheduler::new(intervals());
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
    let mut scheduler = Scheduler::new(Intervals {
        os_core: 0,
        ..intervals()
    });
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
