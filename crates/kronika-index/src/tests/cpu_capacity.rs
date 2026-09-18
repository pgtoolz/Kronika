use super::{RecordedCpuCapacity, cgroup_cpu_capacity};

#[test]
fn cgroup_capacity_preserves_fractions_and_cpuset_limits() {
    assert_eq!(
        cgroup_cpu_capacity(Some(8), Some(150_000), Some(100_000)),
        Some(1.5)
    );
    assert_eq!(
        cgroup_cpu_capacity(Some(2), Some(400_000), Some(100_000)),
        Some(2.0)
    );
    assert_eq!(cgroup_cpu_capacity(Some(2), Some(-1), None), Some(2.0));
    assert_eq!(
        cgroup_cpu_capacity(None, Some(150_000), Some(100_000)),
        Some(1.5)
    );
    for quota in [None, Some(0), Some(-2)] {
        assert_eq!(cgroup_cpu_capacity(Some(8), quota, Some(100_000)), None);
    }
    assert_eq!(cgroup_cpu_capacity(None, Some(-1), Some(100_000)), None);
    for period in [None, Some(0), Some(-1)] {
        assert_eq!(cgroup_cpu_capacity(Some(8), Some(150_000), period), None);
    }
}

#[test]
fn latest_recorded_capacity_can_be_unknown_and_never_uses_future_values() {
    let capacity = RecordedCpuCapacity {
        explicit: None,
        snapshots: [(10, Some(1.5)), (20, Some(2.0)), (30, None)].into(),
    };
    assert_eq!(capacity.at(9), None);
    assert_eq!(capacity.at(10), Some(1.5));
    assert_eq!(capacity.at(19), Some(1.5));
    assert_eq!(capacity.at(20), Some(2.0));
    assert_eq!(capacity.at(30), None);
    assert_eq!(
        RecordedCpuCapacity {
            explicit: Some(3.0),
            ..capacity
        }
        .at(15),
        Some(3.0)
    );
}
