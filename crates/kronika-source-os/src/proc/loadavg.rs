//! Parse `/proc/loadavg` for load averages and process counts (`1_105`).

use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;

use super::stat::ParseError;

/// One snapshot of `/proc/loadavg`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadavgRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// 1-minute load average.
    pub load1: f64,
    /// 5-minute load average.
    pub load5: f64,
    /// 15-minute load average.
    pub load15: f64,
    /// Runnable processes at collection time.
    pub running: i32,
    /// Total threads/processes at collection time.
    pub total: i32,
}

/// Parse the first line of `/proc/loadavg` content.
///
/// Expected format: `0.15 0.10 0.05 2/345 6789`
///
/// # Errors
///
/// Returns [`ParseError`] when a field is missing, a float cannot be parsed,
/// the `running/total` token is malformed, or either integer overflows `i32`.
pub fn parse_loadavg(content: &str, ts: i64) -> Result<LoadavgRow, ParseError> {
    let line = content
        .lines()
        .next()
        .ok_or_else(|| ParseError("/proc/loadavg: empty content".to_owned()))?;

    let mut fields = line.split_whitespace();

    let avgs: [f64; 3] = [
        parse_f64(&mut fields, "load1")?,
        parse_f64(&mut fields, "load5")?,
        parse_f64(&mut fields, "load15")?,
    ];

    let run_total = fields
        .next()
        .ok_or_else(|| ParseError("/proc/loadavg: missing running/total field".to_owned()))?;

    let (run_str, total_str) = run_total.split_once('/').ok_or_else(|| {
        ParseError(format!(
            "/proc/loadavg: expected running/total, got {run_total:?}"
        ))
    })?;

    let running = run_str
        .parse::<i32>()
        .map_err(|e| ParseError(format!("/proc/loadavg: running {run_str:?}: {e}")))?;
    let total = total_str
        .parse::<i32>()
        .map_err(|e| ParseError(format!("/proc/loadavg: total {total_str:?}: {e}")))?;

    Ok(LoadavgRow {
        ts,
        load1: avgs[0],
        load5: avgs[1],
        load15: avgs[2],
        running,
        total,
    })
}

fn parse_f64(
    fields: &mut std::str::SplitWhitespace<'_>,
    name: &'static str,
) -> Result<f64, ParseError> {
    let s = fields
        .next()
        .ok_or_else(|| ParseError(format!("/proc/loadavg: missing field {name:?}")))?;
    s.parse::<f64>()
        .map_err(|e| ParseError(format!("/proc/loadavg: {name} {s:?}: {e}")))
}

impl LoadavgRow {
    /// Registry row for `1_105_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsLoadavg {
        OsLoadavg {
            ts: Ts(self.ts),
            load1: self.load1,
            load5: self.load5,
            load15: self.load15,
            running: self.running,
            total: self.total,
            scope,
        }
    }
}

#[cfg(test)]
#[path = "../tests/proc/loadavg.rs"]
mod tests;
