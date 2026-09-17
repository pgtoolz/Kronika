use super::{SourcePenalty, Stall, health, overall_health, postgres_penalty};

/// One second, in the microseconds the counters use.
const SECOND: i64 = 1_000_000;

const fn stall(cpu: i64, memory: i64, io: i64) -> Stall {
    Stall { cpu, memory, io }
}

const ZERO: Stall = stall(0, 0, 0);

#[test]
fn an_idle_interval_is_a_hundred() {
    assert_eq!(health(ZERO, 0, ZERO, SECOND), Some(100));
}

#[test]
fn a_resource_stalled_the_whole_interval_is_zero() {
    assert_eq!(health(ZERO, 0, stall(SECOND, 0, 0), SECOND), Some(0));
}

#[test]
fn the_worst_resource_decides_and_the_others_do_not_dilute_it() {
    // An idle CPU must not hide a disk that waited for four fifths of the time.
    let after = stall(0, 0, SECOND / 5 * 4);
    assert_eq!(health(ZERO, 0, after, SECOND), Some(20));
}

#[test]
fn each_resource_counts_the_same_way() {
    let quarter = SECOND / 4;
    assert_eq!(health(ZERO, 0, stall(quarter, 0, 0), SECOND), Some(75));
    assert_eq!(health(ZERO, 0, stall(0, quarter, 0), SECOND), Some(75));
    assert_eq!(health(ZERO, 0, stall(0, 0, quarter), SECOND), Some(75));
}

#[test]
fn counters_carry_their_history_so_only_the_delta_matters() {
    let before = stall(700 * SECOND, 12 * SECOND, 5 * SECOND);
    let after = stall(700 * SECOND + SECOND / 2, 12 * SECOND, 5 * SECOND);
    assert_eq!(health(before, 0, after, SECOND), Some(50));
}

#[test]
fn a_counter_that_went_backwards_has_no_health() {
    let before = stall(SECOND, SECOND, SECOND);
    assert_eq!(health(before, 0, ZERO, SECOND), None);
    assert_eq!(health(before, 0, stall(SECOND, 0, SECOND), SECOND), None);
}

#[test]
fn a_zero_or_negative_interval_has_no_health() {
    assert_eq!(health(ZERO, SECOND, ZERO, SECOND), None);
    assert_eq!(health(ZERO, SECOND, ZERO, 0), None);
}

#[test]
fn stalling_longer_than_the_interval_is_zero_not_below() {
    // Cap a counter delta that exceeds the elapsed interval defensively.
    let after = stall(0, 0, 5 * SECOND);
    assert_eq!(health(ZERO, 0, after, SECOND), Some(0));
}

#[test]
fn a_share_rounds_to_the_nearest_whole_percent() {
    // 0.4% and 0.6% of the interval sit either side of half a percent.
    assert_eq!(health(ZERO, 0, stall(4_000, 0, 0), SECOND), Some(100));
    assert_eq!(health(ZERO, 0, stall(6_000, 0, 0), SECOND), Some(99));
}

#[test]
fn a_long_interval_does_not_overflow_the_multiplication() {
    let day = 86_400 * SECOND;
    assert_eq!(health(ZERO, 0, stall(day / 2, 0, 0), day), Some(50));
    assert_eq!(health(ZERO, 0, stall(i64::MAX, 0, 0), day), Some(0));
}

#[test]
fn two_service_slots_per_cpu_set_the_postgres_boundary() {
    assert_eq!(postgres_penalty(0, 2.0), Some(0));
    assert_eq!(postgres_penalty(4, 2.0), Some(0));
    assert_eq!(postgres_penalty(5, 2.0), Some(20));
    assert_eq!(postgres_penalty(8, 2.0), Some(50));
    assert_eq!(postgres_penalty(40, 2.0), Some(90));
}

#[test]
fn postgres_pressure_rounds_to_the_nearest_percent() {
    assert_eq!(postgres_penalty(3, 1.0), Some(33));
    assert_eq!(postgres_penalty(6, 2.0), Some(33));
    assert_eq!(postgres_penalty(7, 2.0), Some(43));
}

#[test]
fn zero_postgres_capacity_is_unknown() {
    assert_eq!(postgres_penalty(0, 0.0), None);
    assert_eq!(postgres_penalty(u32::MAX, 0.0), None);
}

#[test]
fn very_large_postgres_capacity_does_not_overflow() {
    assert_eq!(postgres_penalty(u32::MAX, f64::from(u32::MAX)), Some(0));
}

#[test]
fn overall_health_subtracts_the_postgres_penalty() {
    assert_eq!(overall_health(Some(80), SourcePenalty::Known(20)), Some(60));
}

#[test]
fn overall_health_clamps_additive_penalties_at_zero() {
    assert_eq!(overall_health(Some(40), SourcePenalty::Known(70)), Some(0));
}

#[test]
fn disabled_sources_do_not_reduce_health() {
    assert_eq!(overall_health(Some(73), SourcePenalty::Disabled), Some(73));
}

#[test]
fn enabled_unknown_sources_make_overall_health_unknown() {
    assert_eq!(overall_health(Some(100), SourcePenalty::Unknown), None);
    assert_eq!(overall_health(None, SourcePenalty::Disabled), None);
}

#[test]
fn fractional_postgres_capacity_keeps_the_recorded_service_slots() {
    assert_eq!(postgres_penalty(3, 1.5), Some(0));
    assert_eq!(postgres_penalty(4, 1.5), Some(25));
    for capacity in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert_eq!(postgres_penalty(4, capacity), None);
    }
}

#[test]
fn integer_postgres_capacity_keeps_existing_rounding() {
    for cpus in 1..=64_u32 {
        for active in 1..=2048_u32 {
            let active64 = u64::from(active);
            let waiting = active64.saturating_sub(2 * u64::from(cpus));
            let old = (100 * waiting + active64 / 2) / active64;
            assert_eq!(
                postgres_penalty(active, f64::from(cpus)).map(u64::from),
                Some(old)
            );
        }
    }
}
