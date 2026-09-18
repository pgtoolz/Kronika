//! Time types shared by recorded-data queries.

/// A half-open timestamp range, `[from, to_exclusive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    pub(crate) from: i64,
    pub(crate) to_exclusive: i64,
}

impl TimeRange {
    /// Validate a half-open range without imposing a maximum width.
    ///
    /// # Errors
    ///
    /// Returns an explanation when the upper bound precedes the lower bound.
    pub fn new(from: i64, to_exclusive: i64) -> Result<Self, String> {
        if from > to_exclusive {
            return Err(format!(
                "from ({from}) must not be after to ({to_exclusive})"
            ));
        }
        Ok(Self { from, to_exclusive })
    }

    /// Validate a half-open range under an exact width limit.
    ///
    /// # Errors
    ///
    /// Returns an explanation when subtraction overflows, the range is wider
    /// than `max_width`, or the upper bound precedes the lower bound.
    pub fn bounded(from: i64, to_exclusive: i64, max_width: i64) -> Result<Self, String> {
        let width = to_exclusive.checked_sub(from).ok_or_else(|| {
            format!("window is invalid: to ({to_exclusive}) minus from ({from}) overflows")
        })?;
        if width > max_width {
            return Err(format!(
                "window too wide: to - from is {width} microseconds, the limit is {max_width} microseconds"
            ));
        }
        Self::new(from, to_exclusive)
    }

    /// Inclusive lower timestamp bound.
    #[must_use]
    pub const fn from(self) -> i64 {
        self.from
    }

    /// Exclusive upper timestamp bound.
    #[must_use]
    pub const fn to_exclusive(self) -> i64 {
        self.to_exclusive
    }

    pub(crate) const fn contains(self, timestamp: i64) -> bool {
        timestamp >= self.from && timestamp < self.to_exclusive
    }
}

#[cfg(test)]
#[path = "tests/time.rs"]
mod tests;
