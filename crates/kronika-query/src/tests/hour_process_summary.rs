use super::{Counters, ExactSum, RateSum, Summary, combine, elapsed_seconds, previous_moments};
use std::collections::BTreeMap;

#[test]
fn moments_bind_rates_to_the_immediately_previous_complete_snapshot() {
    let moments = previous_moments(BTreeMap::from([(10, 1), (20, 1), (40, 2)]));
    assert_eq!(
        moments,
        BTreeMap::from([(10, i64::MIN), (20, 10), (40, 20)])
    );
}

#[test]
fn exact_sums_keep_null_distinct_from_zero_and_cover_more_than_one_page() {
    let mut sum = ExactSum::default();
    assert_eq!(sum.value(), None);
    for value in 0..1_005 {
        sum.add(Some(value));
    }
    assert_eq!(sum.value(), Some(504_510.0));
}

#[test]
fn rates_keep_zero_and_reject_missing_resets_and_nonpositive_time() {
    let mut rates = RateSum::default();
    rates.add(Some(10), Some(10), Some(5.0));
    rates.add(Some(20), Some(10), Some(5.0));
    rates.add(None, Some(10), Some(5.0));
    rates.add(Some(5), Some(10), Some(5.0));
    rates.add(Some(20), Some(10), elapsed_seconds(10, 10));
    assert_eq!(rates.value(), Some(2.0));
}

#[test]
fn context_switches_match_the_cards_partial_null_rule() {
    let mut left = RateSum::default();
    let right = RateSum::default();
    assert_eq!(combine(&left, &right), None);
    left.add(Some(20), Some(10), Some(5.0));
    assert_eq!(combine(&left, &right), Some(2.0));
}

#[test]
fn all_sixteen_values_have_stable_positions() {
    let mut summary = Summary {
        processes: 2,
        runnable: 1,
        postgresql: Some(1),
        ticks_per_second: Some(100.0),
        ..Summary::default()
    };
    summary.threads.add(Some(4));
    summary.resident_kib.add(Some(8));
    summary.utime.add(Some(20), Some(10), Some(5.0));
    summary.syscw.add(Some(30), Some(20), Some(5.0));
    let values = summary.values(&super::FIELDS);
    assert_eq!(values.len(), 16);
    assert_eq!(values[0], 2.0);
    assert_eq!(values[1], 4.0);
    assert_eq!(values[2], 1.0);
    assert_eq!(values[3], 1.0);
    assert_eq!(values[4], 0.02);
    assert_eq!(values[8], 8.0);
    assert_eq!(values[15], 2.0);
}

#[test]
fn counter_container_starts_unavailable() {
    let counters = Counters::default();
    assert_eq!(counters.utime, None);
    assert_eq!(counters.read_bytes, None);
}
