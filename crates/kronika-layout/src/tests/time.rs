use super::*;

fn unix_micros(year: i64, month: u8, day: u8) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era - 719_468) * MICROS_PER_DAY
}

#[test]
fn unix_epoch_and_negative_microsecond_use_flooring_utc_days() {
    assert_eq!(
        SegmentId::new(0).unwrap().utc_day().unwrap(),
        UtcDay::new(1970, 1, 1).unwrap()
    );
    assert_eq!(
        SegmentId::new(-1).unwrap().utc_day().unwrap(),
        UtcDay::new(1969, 12, 31).unwrap()
    );
}

#[test]
fn leap_day_and_midnight_boundary_are_exact() {
    let leap_midnight = 1_709_164_800_000_000_i64;
    assert_eq!(
        SegmentId::new(leap_midnight).unwrap().utc_day().unwrap(),
        UtcDay::new(2024, 2, 29).unwrap()
    );
    assert_eq!(
        SegmentId::new(leap_midnight - 1)
            .unwrap()
            .utc_day()
            .unwrap(),
        UtcDay::new(2024, 2, 28).unwrap()
    );
}

#[test]
fn impossible_dates_are_rejected() {
    assert!(UtcDay::new(2023, 2, 29).is_err());
    assert!(UtcDay::new(2024, 2, 29).is_ok());
    assert!(UtcDay::new(2024, 13, 1).is_err());
}

#[test]
fn address_uses_only_the_id_day() {
    let id = SegmentId::new(1_709_164_800_000_000).unwrap();
    let address = SegmentAddress::new(id).unwrap();
    assert_eq!(address.day, UtcDay::new(2024, 2, 29).unwrap());
    assert_eq!(address.zms_name(), "1709164800000000.zms");
}

#[test]
fn supported_year_boundaries_are_exact() {
    let first = unix_micros(0, 1, 1);
    let after_last = unix_micros(10_000, 1, 1);
    assert_eq!(
        SegmentId::new(first).unwrap().utc_day().unwrap(),
        UtcDay::new(0, 1, 1).unwrap()
    );
    assert!(SegmentId::new(first - 1).is_err());
    assert_eq!(
        SegmentId::new(after_last - 1).unwrap().utc_day().unwrap(),
        UtcDay::new(9999, 12, 31).unwrap()
    );
    assert!(SegmentId::new(after_last).is_err());
}

#[test]
fn addresses_round_trip_the_exact_unrounded_identity() {
    for raw in [
        -1,
        0,
        1,
        1_709_164_800_000_001,
        unix_micros(0, 1, 1),
        unix_micros(10_000, 1, 1) - 1,
    ] {
        let id = SegmentId::new(raw).unwrap();
        let address = SegmentAddress::new(id).unwrap();
        assert_eq!(address.id.get(), raw);
        assert_eq!(address.day, id.utc_day().unwrap());
    }
}
