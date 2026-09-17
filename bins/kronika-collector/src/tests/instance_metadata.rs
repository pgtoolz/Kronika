use super::recorded_interval_seconds;

#[test]
fn zero_source_interval_uses_the_timer_tick_as_its_freshness() {
    assert_eq!(recorded_interval_seconds(30, 5), 30);
    assert_eq!(recorded_interval_seconds(0, 5), 5);
    assert_eq!(recorded_interval_seconds(0, 0), 0);
}
