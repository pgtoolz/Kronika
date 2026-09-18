use super::{before_start, overlaps};

/// A segment covering ten to twenty microseconds.
const MIN: i64 = 10;
const MAX: i64 = 20;

#[test]
fn an_unbounded_range_takes_every_segment() {
    assert!(overlaps(&(..), MIN, MAX));
    assert!(overlaps(&(..), i64::MIN, i64::MAX));
}

#[test]
fn a_segment_entirely_before_the_range_is_out() {
    assert!(!overlaps(&(21..), MIN, MAX));
    assert!(!overlaps(&(21..30), MIN, MAX));
}

#[test]
fn predecessor_bounds_follow_range_inclusivity() {
    assert!(before_start(&(21..), 20));
    assert!(!before_start(&(21..), 21));
    assert!(before_start(
        &(std::ops::Bound::Excluded(20), std::ops::Bound::Unbounded),
        20
    ));
    assert!(!before_start(&(..), 20));
}

#[test]
fn a_segment_entirely_after_the_range_is_out() {
    assert!(!overlaps(&(..10), MIN, MAX));
    assert!(!overlaps(&(0..10), MIN, MAX));
}

#[test]
fn a_segment_overlapping_at_one_end_is_in() {
    assert!(overlaps(&(20..), MIN, MAX));
    assert!(overlaps(&(..11), MIN, MAX));
    assert!(overlaps(&(0..15), MIN, MAX));
    assert!(overlaps(&(15..30), MIN, MAX));
}

#[test]
fn a_range_inside_the_segment_is_in() {
    assert!(overlaps(&(12..15), MIN, MAX));
}

#[test]
fn an_excluded_bound_drops_the_touching_segment() {
    // `..=10` keeps a segment starting at 10, `..10` does not.
    assert!(overlaps(&(..=10), MIN, MAX));
    assert!(!overlaps(&(..10), MIN, MAX));
}

#[test]
fn an_instant_segment_is_matched_by_the_instant() {
    assert!(overlaps(&(10..=10), 10, 10));
    assert!(!overlaps(&(10..10), 10, 10));
}

#[test]
fn an_empty_range_takes_nothing() {
    assert!(!overlaps(&(15..15), MIN, MAX));
}

#[test]
fn a_bound_at_the_end_of_the_scale_excludes_everything() {
    use std::ops::Bound;

    // Nothing sits past the last microsecond, or before the first.
    assert!(!overlaps(
        &(Bound::Excluded(i64::MAX), Bound::Unbounded),
        i64::MAX,
        i64::MAX
    ));
    assert!(!overlaps(&(..i64::MIN), i64::MIN, i64::MIN));
}
