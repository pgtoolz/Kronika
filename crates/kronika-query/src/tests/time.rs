use super::TimeRange;

#[test]
fn validates_bounds_and_width() {
    let empty = TimeRange::new(7, 7).expect("empty range");
    assert_eq!((empty.from(), empty.to_exclusive()), (7, 7));

    let range = TimeRange::new(7, 9).expect("range");
    assert_eq!((range.from(), range.to_exclusive()), (7, 9));
    assert_eq!(
        TimeRange::new(9, 7).expect_err("reversed range"),
        "from (9) must not be after to (7)"
    );
    assert_eq!(
        TimeRange::bounded(i64::MIN, i64::MAX, i64::MAX).expect_err("overflowing range"),
        "window is invalid: to (9223372036854775807) minus from (-9223372036854775808) overflows"
    );
    assert!(TimeRange::bounded(7, 10, 2).is_err());
    assert_eq!(TimeRange::bounded(7, 9, 2).expect("bounded range"), range);
}
