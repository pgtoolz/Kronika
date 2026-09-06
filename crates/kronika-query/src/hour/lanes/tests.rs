use std::collections::BTreeMap;

use kronika_reader::{Cell, Row};
use kronika_registry::contract;

use super::{
    ActivitySample, Counters, activity_sample, counter_sum, cpu_busy_ticks, current_points, points,
    rate, record_activity_sample,
};
use super::{Membership, cgroup_cpu_capacity, member_row, membership, pressure_lane};
use crate::Window;

#[test]
fn counter_points_keep_unusable_subtractions_as_null_and_zero_as_data() {
    let stored = BTreeMap::from([(1, 10), (2, 10), (3, 5), (4, 12)]);
    assert_eq!(
        rate(&stored, |value, _seconds| value),
        vec![(1, None), (2, Some(0.0)), (3, None), (4, Some(7.0))]
    );
}

#[test]
fn a_segment_boundary_keeps_the_preceding_counter_reading() {
    let counters = Counters {
        busy_ticks: BTreeMap::from([(100, 10), (200, 20)]),
        ..Counters::default()
    };
    let current = current_points(
        &counters,
        100,
        1,
        200,
        200,
        Window {
            from: Some(200),
            to: Some(200),
        },
    );
    let busy = current
        .iter()
        .find(|point| point.key == "cpu_busy")
        .expect("current busy point");
    assert_eq!(busy.ts, 200);
    assert_eq!(busy.value, Some(100_000.0));
}

#[test]
fn only_the_latest_sample_is_carried_into_the_next_segment() {
    let mut counters = Counters {
        busy_ticks: BTreeMap::from([(1_000_000, 10), (2_000_000, 20)]),
        memory: BTreeMap::from([(1_000_000, 50.0), (2_000_000, 60.0)]),
        swap: BTreeMap::from([(1_000_000, None), (2_000_000, Some(4))]),
        ..Counters::default()
    };

    counters.retain_latest();

    assert_eq!(counters.busy_ticks, BTreeMap::from([(2_000_000, 20)]));
    assert_eq!(counters.memory, BTreeMap::from([(2_000_000, 60.0)]));
    assert_eq!(counters.swap, BTreeMap::from([(2_000_000, Some(4))]));
    counters.busy_ticks.insert(3_000_000, 30);
    let next = current_points(
        &counters,
        100,
        1,
        3_000_000,
        3_000_000,
        Window {
            from: Some(3_000_000),
            to: Some(3_000_000),
        },
    );
    assert_eq!(
        next.iter()
            .find(|point| point.key == "cpu_busy")
            .and_then(|point| point.value),
        Some(10.0)
    );
}

#[test]
fn public_lane_points_stay_inside_the_inclusive_window() {
    let counters = Counters {
        memory: BTreeMap::from([(99, 1.0), (100, 2.0), (200, 3.0), (201, 4.0)]),
        ..Counters::default()
    };
    let current = current_points(
        &counters,
        0,
        0,
        99,
        201,
        Window {
            from: Some(100),
            to: Some(200),
        },
    );

    assert_eq!(
        current
            .iter()
            .map(|point| (point.ts, point.value))
            .collect::<Vec<_>>(),
        [(100, Some(2.0)), (200, Some(3.0))]
    );
}

#[test]
fn cpu_busy_matches_the_aggregate_direct_comparator() {
    let row = row(
        1_102_001,
        &[
            ("user", Cell::I64(100)),
            ("nice", Cell::I64(10)),
            ("system", Cell::I64(20)),
            ("irq", Cell::I64(5)),
            ("softirq", Cell::I64(5)),
            ("steal", Cell::I64(7)),
        ],
    );
    assert_eq!(cpu_busy_ticks(&row), Some(147));
}

#[test]
fn swap_requires_both_counters() {
    let complete = row(
        1_106_001,
        &[("pswpin", Cell::I64(10)), ("pswpout", Cell::I64(20))],
    );
    let incomplete = row(
        1_106_001,
        &[("pswpin", Cell::I64(10)), ("pswpout", Cell::Null)],
    );
    assert_eq!(counter_sum(&complete, &["pswpin", "pswpout"]), Some(30));
    assert_eq!(counter_sum(&incomplete, &["pswpin", "pswpout"]), None);
}

#[test]
fn nullable_swap_and_oom_do_not_bridge_null_samples() {
    let samples = BTreeMap::from([
        (1_000_000, Some(10)),
        (2_000_000, None),
        (3_000_000, Some(30)),
        (4_000_000, Some(40)),
    ]);
    let counters = Counters {
        swap: samples.clone(),
        oom: samples,
        ..Counters::default()
    };

    for key in ["mem_swap", "mem_oom"] {
        let values = points(&counters, 0, 0)
            .into_iter()
            .filter(|point| point.key == key)
            .map(|point| point.value)
            .collect::<Vec<_>>();
        assert_eq!(values, [None, None, None, Some(10.0)], "{key}");
    }
}

#[test]
fn activity_rows_are_reduced_to_the_lane_fields() {
    let row = row(
        1_001_004,
        &[
            ("ts", Cell::Ts(5_000_000)),
            ("backend_type", Cell::StrId(11)),
            ("state", Cell::StrId(12)),
            ("wait_event_type", Cell::StrId(13)),
            ("leader_pid", Cell::I32(42)),
            ("xact_start", Cell::Ts(3_000_000)),
        ],
    );

    assert_eq!(
        activity_sample(&row),
        ActivitySample {
            ts: Some(5_000_000),
            backend_type: Some(11),
            state: Some(12),
            wait_event_type: Some(13),
            leader: true,
            xact_start: Some(3_000_000),
        }
    );
}

#[test]
fn lock_waits_have_a_lane_distinct_from_other_backend_waits() {
    let counters = Counters {
        waiting: BTreeMap::from([(5_000_000, 1.0)]),
        lock_waiting: BTreeMap::from([(5_000_000, 0.0)]),
        ..Counters::default()
    };

    let lanes = points(&counters, 0, 0);
    assert_eq!(
        lanes
            .iter()
            .find(|point| point.key == "pg_waiting")
            .and_then(|point| point.value),
        Some(1.0)
    );
    assert_eq!(
        lanes
            .iter()
            .find(|point| point.key == "pg_lock_waiting")
            .and_then(|point| point.value),
        Some(0.0)
    );
}

#[test]
fn background_lock_waits_keep_the_lock_graph_visible() {
    let mut counters = Counters::default();
    let sample = ActivitySample {
        ts: Some(5_000_000),
        backend_type: Some(11),
        state: Some(12),
        wait_event_type: Some(13),
        leader: false,
        xact_start: None,
    };

    record_activity_sample(
        &mut counters,
        &sample,
        Some(b"autovacuum worker"),
        Some(b"active"),
        Some(b"Lock"),
    );

    assert_eq!(counters.lock_waiting, BTreeMap::from([(5_000_000, 1.0)]));
    assert_eq!(counters.waiting, BTreeMap::from([(5_000_000, 0.0)]));
}

fn row(type_id: u32, values: &[(&str, Cell)]) -> Row {
    let contract = contract(type_id).expect("fixture contract");
    let cells = contract
        .columns
        .iter()
        .map(|column| {
            values
                .iter()
                .find_map(|(name, value)| (*name == column.name).then(|| value.clone()))
                .unwrap_or(Cell::Null)
        })
        .collect();
    Row::new(contract, cells)
}

#[test]
fn a_shared_boundary_row_is_not_emitted_again_by_the_next_segment() {
    // Adjacent segments share the snapshot row at ts 200. The first segment
    // emits it with a computed rate; after retain_latest the next segment
    // holds only that row, whose rate is null — re-emitting it would conflict
    // with the value already sent.
    let mut counters = Counters {
        busy_ticks: BTreeMap::from([(100, 10), (200, 20)]),
        ..Counters::default()
    };
    let window = Window {
        from: Some(0),
        to: Some(1_000),
    };
    let first = current_points(&counters, 100, 1, 100, 200, window);
    assert!(
        first
            .iter()
            .any(|point| point.key == "cpu_busy" && point.ts == 200 && point.value.is_some())
    );
    counters.retain_latest();
    counters.busy_ticks.insert(200, 20);
    counters.busy_ticks.insert(300, 30);
    let second = current_points(&counters, 100, 1, 200_i64.saturating_add(1), 300, window);
    assert!(second.iter().all(|point| point.ts != 200));
    assert!(
        second
            .iter()
            .any(|point| point.key == "cpu_busy" && point.ts == 300 && point.value.is_some())
    );
}

#[test]
fn container_cpu_lanes_measure_the_collector_cgroup_against_its_capacity() {
    let counters = Counters {
        cg_cpu_usage: BTreeMap::from([(1_000_000, 0), (2_000_000, 500_000)]),
        cg_cpu_throttled: BTreeMap::from([(1_000_000, 0), (2_000_000, 250_000)]),
        cg_cpu_capacity: BTreeMap::from([(1_000_000, 2.0)]),
        ..Counters::default()
    };
    let out = points(&counters, 100, 4);
    let at_two = |key: &str| {
        out.iter()
            .find(|point| point.key == key && point.ts == 2_000_000)
            .and_then(|point| point.value)
    };
    assert_eq!(at_two("cg_cpu_cores"), Some(0.5));
    assert_eq!(at_two("cg_cpu_share"), Some(25.0));
    assert_eq!(at_two("cg_cpu_throttle"), Some(25.0));
    // Without a recorded capacity there is no share lane: four host cores never substitute.
    let unlimited = Counters {
        cg_cpu_usage: counters.cg_cpu_usage,
        ..Counters::default()
    };
    let out = points(&unlimited, 100, 4);
    assert!(out.iter().all(|point| point.key != "cg_cpu_share"));
    assert!(out.iter().any(|point| point.key == "cg_cpu_cores"));
}

#[test]
fn container_gauges_events_and_io_have_their_own_lanes() {
    let counters = Counters {
        cg_memory_share: BTreeMap::from([(1_000_000, 40.0)]),
        cg_memory_bytes: BTreeMap::from([(1_000_000, 1024.0)]),
        cg_pids: BTreeMap::from([(1_000_000, 4.0)]),
        cg_pids_share: BTreeMap::from([(1_000_000, 3.125)]),
        cg_oom: BTreeMap::from([
            (1_000_000, Some(1)),
            (2_000_000, Some(3)),
            (3_000_000, None),
        ]),
        cg_io_read: BTreeMap::from([(1_000_000, 0), (2_000_000, 4096)]),
        cg_io_write: BTreeMap::from([(1_000_000, 0), (2_000_000, 8192)]),
        cg_stall_memory: BTreeMap::from([(1_000_000, 0), (2_000_000, 100_000)]),
        ..Counters::default()
    };
    let out = points(&counters, 100, 1);
    let value = |key: &str, ts: i64| {
        out.iter()
            .find(|point| point.key == key && point.ts == ts)
            .map(|point| point.value)
    };
    assert_eq!(value("cg_memory", 1_000_000), Some(Some(40.0)));
    assert_eq!(value("cg_memory_bytes", 1_000_000), Some(Some(1024.0)));
    assert_eq!(value("cg_pids", 1_000_000), Some(Some(4.0)));
    assert_eq!(value("cg_pids_share", 1_000_000), Some(Some(3.125)));
    assert_eq!(value("cg_oom", 2_000_000), Some(Some(2.0)));
    assert_eq!(
        value("cg_oom", 3_000_000),
        Some(None),
        "a null sample breaks the OOM rate"
    );
    assert_eq!(value("cg_io_read", 2_000_000), Some(Some(4096.0)));
    assert_eq!(value("cg_io_write", 2_000_000), Some(Some(8192.0)));
    assert_eq!(value("cg_mem_psi", 2_000_000), Some(Some(10.0)));
}

#[test]
fn container_pressure_stays_apart_from_host_pressure() {
    let mut counters = Counters::default();
    pressure_lane(&mut counters, 0, 0)
        .expect("host cpu pressure")
        .insert(1, 10);
    pressure_lane(&mut counters, 3, 0)
        .expect("container cpu pressure")
        .insert(1, 20);
    pressure_lane(&mut counters, 3, 1)
        .expect("container memory pressure")
        .insert(1, 30);
    pressure_lane(&mut counters, 3, 2)
        .expect("container io pressure")
        .insert(1, 40);
    assert!(
        pressure_lane(&mut counters, 0, 1).is_none(),
        "host memory pressure has no lane"
    );
    assert!(
        pressure_lane(&mut counters, 1, 0).is_none(),
        "a pod scope is not a lane"
    );
    assert_eq!(counters.stall_cpu, BTreeMap::from([(1, 10)]));
    assert_eq!(counters.stall_io, BTreeMap::new());
    assert_eq!(counters.cg_stall_cpu, BTreeMap::from([(1, 20)]));
    assert_eq!(counters.cg_stall_memory, BTreeMap::from([(1, 30)]));
    assert_eq!(counters.cg_stall_io, BTreeMap::from([(1, 40)]));
}

#[test]
fn the_membership_selects_rows_by_exact_path_and_scope() {
    let context = row(
        1_205_001,
        &[
            ("cgroup_version", Cell::U32(2)),
            ("cpu_path", Cell::StrId(7)),
            ("memory_path", Cell::StrId(7)),
            ("io_path", Cell::StrId(7)),
            ("cpuset_cpus", Cell::I64(4)),
            ("effective_cpu_quota_usec", Cell::I64(150_000)),
            ("effective_cpu_period_usec", Cell::I64(100_000)),
            ("scope", Cell::U32(3)),
        ],
    );
    let unified = membership(&context);
    assert_eq!(
        unified,
        Membership {
            scope: Some(3),
            cpu: Some(7),
            memory: Some(7),
            io: Some(7),
            pids: Some(7),
        }
    );
    assert_eq!(cgroup_cpu_capacity(&context), Some(1.5));
    let unlimited = row(
        1_205_001,
        &[
            ("cgroup_version", Cell::U32(1)),
            ("cpu_path", Cell::StrId(7)),
            ("memory_path", Cell::StrId(8)),
            ("io_path", Cell::StrId(7)),
            ("cpuset_cpus", Cell::I64(2)),
            ("effective_cpu_quota_usec", Cell::I64(-1)),
            ("effective_cpu_period_usec", Cell::I64(100_000)),
        ],
    );
    assert_eq!(
        membership(&unlimited).pids,
        None,
        "v1 controllers have no unified TID row"
    );
    assert_eq!(
        cgroup_cpu_capacity(&unlimited),
        Some(2.0),
        "an unlimited quota leaves the cpuset"
    );
    let unknown = row(1_205_001, &[("cgroup_version", Cell::U32(2))]);
    assert_eq!(
        cgroup_cpu_capacity(&unknown),
        None,
        "no quota and no cpuset is unknown, not host cores"
    );

    let mine = row(
        1_201_001,
        &[("cgroup_path", Cell::StrId(7)), ("scope", Cell::U32(3))],
    );
    let other_path = row(
        1_201_001,
        &[("cgroup_path", Cell::StrId(9)), ("scope", Cell::U32(3))],
    );
    let other_scope = row(
        1_201_001,
        &[("cgroup_path", Cell::StrId(7)), ("scope", Cell::U32(4))],
    );
    assert!(member_row(&mine, Some(&unified), unified.cpu));
    assert!(!member_row(&other_path, Some(&unified), unified.cpu));
    assert!(!member_row(&other_scope, Some(&unified), unified.cpu));
    assert!(
        !member_row(&mine, None, unified.cpu),
        "no context selects nothing"
    );
}

#[test]
fn a_v2_membership_keeps_threads_when_a_controller_is_unavailable() {
    for available in ["cpu_path", "memory_path", "io_path"] {
        let context = row(
            1_205_001,
            &[
                ("cgroup_version", Cell::U32(2)),
                (available, Cell::StrId(7)),
                ("scope", Cell::U32(3)),
            ],
        );
        let own = membership(&context);
        assert_eq!(own.pids, Some(7), "{available}");
        let threads = row(
            1_204_001,
            &[("cgroup_path", Cell::StrId(7)), ("scope", Cell::U32(3))],
        );
        assert!(member_row(&threads, Some(&own), own.pids));
    }
    let no_controller = row(1_205_001, &[("cgroup_version", Cell::U32(2))]);
    assert_eq!(membership(&no_controller).pids, None);
}
