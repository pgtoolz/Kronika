//! Heatmap time buckets and observation coverage.

use super::{CellSum, CounterCell, GridCells, MAX_COUNTER_GAP_US, Obs};

use crate::heatmap::result::HeatmapInterval;

pub(in crate::heatmap) fn intervals(
    range: crate::TimeRange,
    columns: usize,
) -> Vec<HeatmapInterval> {
    (0..columns)
        .map(|index| HeatmapInterval {
            start: interval_start(range, columns, index),
            end: interval_start(range, columns, index + 1).saturating_sub(1),
        })
        .collect()
}

fn interval_start(range: crate::TimeRange, columns: usize, index: usize) -> i64 {
    let span = i128::from(range.to_exclusive) - i128::from(range.from);
    let offset = span * to_i128(index) / to_i128(columns.max(1));
    range.from.saturating_add(clamped(offset))
}

pub(in crate::heatmap) fn column_of_span(
    previous_ts: Option<i64>,
    timestamp: i64,
    range: crate::TimeRange,
    columns: usize,
) -> usize {
    let middle = match previous_ts {
        Some(previous) if previous < timestamp => previous + (timestamp - previous) / 2,
        _ => timestamp,
    };
    column_of(middle.max(range.from), range, columns)
}

pub(in crate::heatmap) fn column_of(
    timestamp: i64,
    range: crate::TimeRange,
    columns: usize,
) -> usize {
    let span = (i128::from(range.to_exclusive) - i128::from(range.from)).max(1);
    let offset = i128::from(timestamp) - i128::from(range.from);
    let column = (offset * to_i128(columns.max(1)) / span).max(0);
    usize::try_from(column)
        .unwrap_or_else(|_| columns.saturating_sub(1))
        .min(columns.saturating_sub(1))
}

fn to_i128(value: usize) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

fn clamped(offset: i128) -> i64 {
    i64::try_from(offset).unwrap_or(i64::MAX)
}

impl GridCells {
    pub(super) fn new(columns: usize, cumulative: bool) -> Self {
        if cumulative {
            Self::Counters(vec![CounterCell::default(); columns])
        } else {
            Self::Gauges(vec![Obs::default(); columns])
        }
    }

    pub(super) fn values(&self) -> impl Iterator<Item = Option<f64>> + '_ {
        let count = match self {
            Self::Counters(cells) => cells.len(),
            Self::Gauges(cells) => cells.len(),
        };
        (0..count).map(|column| match self {
            Self::Counters(cells) => cells[column].value(),
            Self::Gauges(cells) => cells[column].cell(false),
        })
    }

    pub(super) fn observe_span(
        &mut self,
        previous: (i64, f64),
        current: (i64, f64),
        range: crate::TimeRange,
    ) {
        let Self::Counters(cells) = self else {
            return;
        };
        let Some(elapsed) = current
            .0
            .checked_sub(previous.0)
            .filter(|elapsed| (1..=MAX_COUNTER_GAP_US).contains(elapsed))
            .and_then(|elapsed| u32::try_from(elapsed).ok())
        else {
            return;
        };
        let delta = current.1 - previous.1;
        if delta < 0.0 {
            return;
        }
        let column = column_of_span(Some(previous.0), current.0, range, cells.len());
        cells[column].delta += delta;
        cells[column].elapsed_us += f64::from(elapsed);
    }
}

impl CounterCell {
    fn value(self) -> Option<f64> {
        (self.elapsed_us > 0.0).then(|| self.delta / (self.elapsed_us / 1_000_000.0))
    }
}

impl Obs {
    pub(in crate::heatmap) fn observe(&mut self, timestamp: i64, value: f64) {
        if self.count == 0 {
            *self = Self {
                count: 1,
                first_ts: timestamp,
                first_value: value,
                last_ts: timestamp,
                last_value: value,
                max_value: value,
            };
            return;
        }
        self.count = self.count.saturating_add(1);
        if timestamp < self.first_ts {
            self.first_ts = timestamp;
            self.first_value = value;
        }
        if timestamp >= self.last_ts {
            self.last_ts = timestamp;
            self.last_value = value;
        }
        if value > self.max_value {
            self.max_value = value;
        }
    }

    pub(in crate::heatmap) fn cell(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.last_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        if delta < 0.0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "an interval of 2^52 microseconds is 142 years"
        )]
        let seconds = (self.last_ts - self.first_ts) as f64 / 1_000_000.0;
        Some(delta / seconds)
    }

    pub(in crate::heatmap) fn total(&self, cumulative: bool) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        if !cumulative {
            return Some(self.max_value);
        }
        if self.count < 2 || self.last_ts <= self.first_ts {
            return None;
        }
        let delta = self.last_value - self.first_value;
        (delta >= 0.0).then_some(delta)
    }
}

impl CellSum {
    pub(in crate::heatmap) fn add(&mut self, value: f64) {
        self.sum += value;
        self.contributors = self.contributors.saturating_add(1);
    }

    pub(in crate::heatmap) fn value(&self) -> Option<f64> {
        (self.contributors > 0).then_some(self.sum)
    }

    pub(in crate::heatmap) fn minus(&self, winners: &Self) -> Option<f64> {
        let contributors = self.contributors.saturating_sub(winners.contributors);
        (contributors > 0).then_some(self.sum - winners.sum)
    }
}

pub(in crate::heatmap) fn band_peak(cells: &[CellSum]) -> Option<f64> {
    cells
        .iter()
        .filter_map(CellSum::value)
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}

pub(in crate::heatmap) fn peak_values(cells: &[Option<f64>]) -> Option<f64> {
    cells
        .iter()
        .flatten()
        .copied()
        .fold(None, |current, value| {
            Some(current.map_or(value, |stored: f64| stored.max(value)))
        })
}
