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

    counters.retain_after(i64::MAX);

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
    counters.retain_after(i64::MAX);
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
        cg_cpu_capacity: BTreeMap::from([(1_000_000, Some(2.0))]),
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
        cg_memory_share: BTreeMap::from([(1_000_000, Some(40.0))]),
        cg_memory_bytes: BTreeMap::from([(1_000_000, 1024.0)]),
        cg_pids: BTreeMap::from([(1_000_000, 4.0)]),
        cg_pids_share: BTreeMap::from([(1_000_000, Some(3.125))]),
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

#[test]
fn selected_context_uses_each_observed_cpu_bound_without_changing_legacy_rules() {
    let fields = [("cpuset_cpus", Cell::I64(8))];
    assert_eq!(cgroup_cpu_capacity(&row(1_205_002, &fields)), Some(8.0));
    assert_eq!(cgroup_cpu_capacity(&row(1_205_001, &fields)), None);
    let fields = [
        ("cpuset_cpus", Cell::I64(8)),
        ("effective_cpu_quota_usec", Cell::I64(150_000)),
        ("effective_cpu_period_usec", Cell::I64(100_000)),
    ];
    assert_eq!(cgroup_cpu_capacity(&row(1_205_002, &fields)), Some(1.5));
    assert_eq!(cgroup_cpu_capacity(&row(1_205_002, &[])), None);
}

#[test]
fn overlapping_portions_keep_finalized_predecessor_and_pending_samples() {
    let mut counters = Counters {
        cg_cpu_usage: BTreeMap::from([
            (1_000_000, 0),
            (2_000_000, 500_000),
            (3_000_000, 1_000_000),
        ]),
        ..Counters::default()
    };
    counters.retain_after(1_999_999);
    assert_eq!(counters.cg_cpu_usage.len(), 3);
    let output = current_points(&counters, 0, 0, 2_000_000, 3_000_000, Window::default());
    let cpu = output
        .iter()
        .filter(|point| point.key == "cg_cpu_cores")
        .map(|point| (point.ts, point.value))
        .collect::<Vec<_>>();
    assert_eq!(cpu, [(2_000_000, Some(0.5)), (3_000_000, Some(0.5))]);
    counters.retain_after(2_999_999);
    assert_eq!(
        counters.cg_cpu_usage.keys().copied().collect::<Vec<_>>(),
        [2_000_000, 3_000_000]
    );
}

#[test]
fn deferred_host_cpu_uses_its_own_recorded_units() {
    let counters = Counters {
        busy_ticks: BTreeMap::from([(1_000_000, 0), (2_000_000, 100)]),
        cpu_units: BTreeMap::from([(1_000_000, (100, 2)), (2_000_000, (100, 2))]),
        ..Counters::default()
    };
    let output = current_points(&counters, 0, 0, 2_000_000, 2_000_000, Window::default());
    assert_eq!(
        output
            .iter()
            .find(|point| point.key == "cpu_busy")
            .and_then(|point| point.value),
        Some(50.0)
    );
}

#[test]
fn selected_io_portions_do_not_count_a_repeated_device_snapshot_twice() {
    let mut counters = Counters::default();
    let device = |minor, bytes| {
        row(
            1_203_003,
            &[
                ("ts", Cell::Ts(1_000_000)),
                ("cgroup_path", Cell::StrId(7)),
                ("cgroup_identity", Cell::StrId(8)),
                ("major", Cell::U32(8)),
                ("minor", Cell::U32(minor)),
                ("rbytes", Cell::I64(bytes)),
            ],
        )
    };
    super::record_cgroup_io(&mut counters, &device(0, 100));
    super::record_cgroup_io(&mut counters, &device(0, 100));
    super::record_cgroup_io(&mut counters, &device(1, 200));
    assert_eq!(counters.cg_io_read, BTreeMap::from([(1_000_000, 300)]));
    assert!(counters.cg_io_write.is_empty());
}

#[test]
fn overlapping_identity_observations_keep_true_transition_only() {
    let a = [Some(1); 4];
    let b = [Some(2); 4];
    let mut identities = BTreeMap::from([(10_000_000, a), (30_000_000, b)]);
    let mut counters = Counters {
        cg_cpu_usage: BTreeMap::from([(10_000_000, 0), (30_000_000, 10_000_000)]),
        ..Counters::default()
    };
    super::refresh_identity_boundaries(&mut counters, &identities, i64::MIN);
    counters.retain_after(19_999_999);
    super::retain_after(&mut identities, 19_999_999);
    identities.insert(20_000_000, a);
    counters.cg_cpu_usage.insert(20_000_000, 5_000_000);
    super::refresh_identity_boundaries(&mut counters, &identities, 19_999_999);
    assert_eq!(
        counters
            .cg_cpu_boundaries
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [30_000_000]
    );
    let output = current_points(&counters, 0, 0, 20_000_000, 30_000_000, Window::default());
    let cpu = output
        .iter()
        .filter(|point| point.key == "cg_cpu_cores")
        .map(|point| (point.ts, point.value))
        .collect::<Vec<_>>();
    assert_eq!(cpu, [(20_000_000, Some(0.5)), (30_000_000, None)]);
}

#[test]
fn disk_winners_keep_exact_deltas_queue_identity_and_membership() {
    use super::{DiskCounters, DiskIdentity};
    let disk = |major, name: &str, busy, weighted| DiskCounters {
        identity: DiskIdentity {
            major,
            minor: 0,
            name: Some(name.into()),
            scope: Some(0),
        },
        busy,
        weighted,
    };
    let mut counters = Counters::default();
    counters.disks.insert(
        1_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, "sda", Some(100), Some(100))),
            ((253, 0), disk(253, "dm-0", Some(200), Some(200))),
        ]),
    );
    counters.disks.insert(
        2_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, "sda", Some(1200), Some(4000))),
            ((253, 0), disk(253, "dm-0", Some(1500), Some(400))),
        ]),
    );
    counters.disks.insert(
        3_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, "sda", Some(1500), Some(4100))),
            ((253, 0), disk(253, "dm-0", Some(1600), Some(9000))),
        ]),
    );
    // Equal zero rates still identify one device. A reset of its queue remains null.
    counters.disks.insert(
        4_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, "sda", Some(1500), Some(1))),
            ((253, 0), disk(253, "dm-0", Some(1600), Some(9000))),
        ]),
    );
    counters.disks.insert(
        5_000_000,
        BTreeMap::from([((253, 0), disk(253, "dm-0", None, Some(9500)))]),
    );
    counters.disks.insert(
        6_000_000,
        BTreeMap::from([((8, 0), disk(8, "renamed", Some(2000), Some(500)))]),
    );
    counters.disks.insert(
        7_000_000,
        BTreeMap::from([((8, 0), disk(8, "renamed", Some(1), Some(501)))]),
    );
    let output = points(&counters, 0, 0);
    let actual: Vec<_> = output
        .iter()
        .filter(|point| point.key == "disk_busy")
        .map(|point| {
            (
                point.ts,
                point.value,
                point.device.as_ref().map(|device| device.name.as_deref()),
            )
        })
        .collect();
    assert_eq!(
        actual,
        [
            (1_000_000, None, None),
            (2_000_000, Some(130.0), Some(Some("dm-0"))),
            (3_000_000, Some(30.0), Some(Some("sda"))),
            (4_000_000, Some(0.0), Some(Some("sda"))),
            (5_000_000, None, None),
            (6_000_000, None, None),
            (7_000_000, None, None),
        ]
    );
    let queues: Vec<_> = output
        .iter()
        .filter(|point| point.key == "disk_queue")
        .map(|point| point.value)
        .collect();
    assert_eq!(queues, [None, Some(0.2), Some(0.1), None, None, None, None]);
    counters.retain_after(5_000_000);
    assert_eq!(
        counters.disks.keys().copied().collect::<Vec<_>>(),
        [5_000_000, 6_000_000, 7_000_000]
    );
}

#[test]
fn disk_tie_names_the_device_beneath_the_others() {
    use std::collections::BTreeSet;

    use super::{DiskCounters, DiskIdentity};
    let disk = |major, minor, name: &str, busy| DiskCounters {
        identity: DiskIdentity {
            major,
            minor,
            name: Some(name.into()),
            scope: Some(0),
        },
        busy: Some(busy),
        weighted: Some(busy),
    };
    let mut counters = Counters::default();
    // dm-0 sits on partition 259:4 (no diskstats row of its own), which sits on nvme0n1.
    counters
        .disk_parents
        .insert((252, 0), BTreeSet::from([(259, 4)]));
    counters
        .disk_parents
        .insert((259, 4), BTreeSet::from([(259, 0)]));
    counters.disks.insert(
        1_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, 0, "sda", 0)),
            ((8, 16), disk(8, 16, "sdb", 0)),
            ((252, 0), disk(252, 0, "dm-0", 0)),
            ((259, 0), disk(259, 0, "nvme0n1", 0)),
        ]),
    );
    // The same request on every layer: dm-0 and nvme0n1 tie, and the lower number is dm-0.
    counters.disks.insert(
        2_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, 0, "sda", 100)),
            ((8, 16), disk(8, 16, "sdb", 100)),
            ((252, 0), disk(252, 0, "dm-0", 300)),
            ((259, 0), disk(259, 0, "nvme0n1", 300)),
        ]),
    );
    // Two disks with no stack between them tie: the lower number stays.
    counters.disks.insert(
        3_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, 0, "sda", 500)),
            ((8, 16), disk(8, 16, "sdb", 500)),
            ((252, 0), disk(252, 0, "dm-0", 400)),
            ((259, 0), disk(259, 0, "nvme0n1", 400)),
        ]),
    );
    // A volume busier than its disk still wins outright.
    counters.disks.insert(
        4_000_000,
        BTreeMap::from([
            ((8, 0), disk(8, 0, "sda", 500)),
            ((8, 16), disk(8, 16, "sdb", 500)),
            ((252, 0), disk(252, 0, "dm-0", 900)),
            ((259, 0), disk(259, 0, "nvme0n1", 800)),
        ]),
    );
    let output = points(&counters, 0, 0);
    let actual: Vec<_> = output
        .iter()
        .filter(|point| point.key == "disk_busy")
        .map(|point| {
            (
                point.ts,
                point.value,
                point
                    .device
                    .as_ref()
                    .and_then(|device| device.name.as_deref()),
            )
        })
        .collect();
    assert_eq!(
        actual,
        [
            (1_000_000, None, None),
            (2_000_000, Some(30.0), Some("nvme0n1")),
            (3_000_000, Some(40.0), Some("sda")),
            (4_000_000, Some(50.0), Some("dm-0")),
        ]
    );
}

#[test]
fn disk_counter_subtraction_precedes_float_conversion() {
    use super::{DiskCounters, DiskIdentity};
    let mut counters = Counters::default();
    for (ts, busy) in [
        (1_000_000, 9_007_199_254_740_993),
        (2_000_000, 9_007_199_254_740_994),
    ] {
        counters.disks.insert(
            ts,
            BTreeMap::from([(
                (8, 0),
                DiskCounters {
                    identity: DiskIdentity {
                        major: 8,
                        minor: 0,
                        name: Some("sda".into()),
                        scope: Some(0),
                    },
                    busy: Some(busy),
                    weighted: Some(busy),
                },
            )]),
        );
    }
    let points = points(&counters, 0, 0);
    assert_eq!(
        points
            .iter()
            .find(|point| point.key == "disk_busy" && point.ts == 2_000_000)
            .expect("busy")
            .value,
        Some(0.1)
    );
    assert_eq!(
        points
            .iter()
            .find(|point| point.key == "disk_queue" && point.ts == 2_000_000)
            .expect("queue")
            .value,
        Some(0.001)
    );
}
