use std::fmt;

use crate::LayoutError;

const MICROS_PER_DAY: i64 = 86_400_000_000;

/// Unix microseconds of the first window successfully appended to a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentId(i64);

impl SegmentId {
    /// Validates a raw Unix-microsecond timestamp against the layout's
    /// representable UTC year range.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::SegmentIdOutOfRange`] outside years
    /// `0000..=9999`.
    pub fn new(value: i64) -> Result<Self, LayoutError> {
        let id = Self(value);
        id.utc_day()?;
        Ok(id)
    }

    /// Returns the Unix-microsecond value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Derives the canonical UTC calendar bucket.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::SegmentIdOutOfRange`] when the timestamp maps
    /// outside years `0000..=9999`.
    pub fn utc_day(self) -> Result<UtcDay, LayoutError> {
        let days = self.0.div_euclid(MICROS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        if !(0..=9999).contains(&year) {
            return Err(LayoutError::SegmentIdOutOfRange(self.0));
        }
        let year =
            u16::try_from(year).map_err(|_overflow| LayoutError::SegmentIdOutOfRange(self.0))?;
        Ok(UtcDay { year, month, day })
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One valid UTC day in the on-disk calendar tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcDay {
    /// Four-digit year, `0000..=9999`.
    pub year: u16,
    /// Month, `1..=12`.
    pub month: u8,
    /// Day of month.
    pub day: u8,
}

impl UtcDay {
    /// Constructs and validates a calendar day.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::MalformedDayDirectory`] for an impossible date.
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, LayoutError> {
        if year > 9999 || !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month)
        {
            return Err(LayoutError::MalformedDayDirectory {
                name: format!("{year:04}/{month:02}/{day:02}"),
            });
        }
        Ok(Self { year, month, day })
    }

    /// Returns the four-digit year component.
    #[must_use]
    pub fn year_component(self) -> String {
        format!("{:04}", self.year)
    }

    /// Returns the two-digit month component.
    #[must_use]
    pub fn month_component(self) -> String {
        format!("{:02}", self.month)
    }

    /// Returns the two-digit day component.
    #[must_use]
    pub fn day_component(self) -> String {
        format!("{:02}", self.day)
    }
}

impl fmt::Display for UtcDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}/{:02}/{:02}", self.year, self.month, self.day)
    }
}

/// Verified physical address of one finished segment and its derived sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SegmentAddress {
    /// Stable segment identity.
    pub id: SegmentId,
    /// UTC bucket derived from `id`.
    pub day: UtcDay,
}

impl SegmentAddress {
    /// Creates the only valid address for `id`.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::SegmentIdOutOfRange`] when `id` is not
    /// representable by the layout.
    pub fn new(id: SegmentId) -> Result<Self, LayoutError> {
        Ok(Self {
            id,
            day: id.utc_day()?,
        })
    }

    /// Validates that an already parsed day matches `id`.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError::MisbucketedSegment`] on mismatch.
    pub fn in_day(id: SegmentId, day: UtcDay) -> Result<Self, LayoutError> {
        let expected = id.utc_day()?;
        if expected != day {
            return Err(LayoutError::MisbucketedSegment {
                id,
                actual: day,
                expected,
            });
        }
        Ok(Self { id, day })
    }

    /// Returns the canonical ZMS file name.
    #[must_use]
    pub fn zms_name(self) -> String {
        format!("{}.zms", self.id)
    }

    /// Returns the canonical IDX file name.
    #[must_use]
    pub fn idx_name(self) -> String {
        format!("{}.idx", self.id)
    }
}

const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u8, u8) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (
        year,
        u8::try_from(month).expect("civil month"),
        u8::try_from(day).expect("civil day"),
    )
}

#[cfg(test)]
mod tests {
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
}
