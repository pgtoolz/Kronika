//! Parsers for cgroup controller files.

use super::DEFAULT_CPU_PERIOD_USEC;
use super::model::{CgroupCpuRow, CgroupIoRow, CgroupMemoryRow};

/// Parse cgroup v2 `cpu.max`.
#[must_use]
pub fn parse_cpu_max(content: &str) -> (i64, i64) {
    let mut fields = content.split_whitespace();
    let quota = match fields.next() {
        Some("max") | None => -1,
        Some(value) => value.parse().unwrap_or(-1),
    };
    let period = fields
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_CPU_PERIOD_USEC);
    (quota, period)
}

/// Parse cgroup v2 `cpu.stat`.
#[must_use]
pub fn parse_cpu_stat(content: &str, ts: i64, cgroup_path: &str) -> CgroupCpuRow {
    let mut row = CgroupCpuRow {
        ts,
        cgroup_path: cgroup_path.to_owned(),
        usage_usec: 0,
        user_usec: 0,
        system_usec: 0,
        throttled_usec: 0,
        nr_throttled: 0,
        quota_usec: -1,
        period_usec: DEFAULT_CPU_PERIOD_USEC,
    };
    for (key, value) in key_value_lines(content) {
        match key {
            "usage_usec" => row.usage_usec = value,
            "user_usec" => row.user_usec = value,
            "system_usec" => row.system_usec = value,
            "throttled_usec" => row.throttled_usec = value,
            "nr_throttled" => row.nr_throttled = value,
            _ => {}
        }
    }
    row
}

pub(super) fn parse_memory_stat_v2(content: &str, row: &mut CgroupMemoryRow) {
    for (key, value) in key_value_lines(content) {
        match key {
            "anon" => row.anon = value,
            "file" => row.file = value,
            "kernel" => row.kernel = value,
            "slab" => row.slab = value,
            _ => {}
        }
    }
}

pub(super) fn parse_memory_events(content: &str, row: &mut CgroupMemoryRow) {
    for (key, value) in key_value_lines(content) {
        match key {
            "low" => row.low_events = value,
            "high" => row.high_events = value,
            "max" => row.max_events = value,
            "oom" => row.oom_events = value,
            "oom_kill" => row.oom_kill = value,
            _ => {}
        }
    }
}

/// Parse cgroup v2 `io.stat`.
#[must_use]
pub fn parse_io_stat(content: &str, ts: i64, cgroup_path: &str) -> Vec<CgroupIoRow> {
    parse_io_stat_bounded(content, ts, cgroup_path, usize::MAX).unwrap_or_default()
}

pub(super) fn parse_io_stat_bounded(
    content: &str,
    ts: i64,
    cgroup_path: &str,
    limit: usize,
) -> Option<Vec<CgroupIoRow>> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let Some(device) = fields.next() else {
            continue;
        };
        let Some((major, minor)) = parse_device(device) else {
            continue;
        };
        let mut row = CgroupIoRow {
            ts,
            cgroup_path: cgroup_path.to_owned(),
            major,
            minor,
            rbytes: None,
            wbytes: None,
            rios: None,
            wios: None,
        };
        for field in fields {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let Ok(value) = value.parse() else {
                continue;
            };
            match key {
                "rbytes" => {
                    row.rbytes = Some(value);
                }
                "wbytes" => {
                    row.wbytes = Some(value);
                }
                "rios" => {
                    row.rios = Some(value);
                }
                "wios" => {
                    row.wios = Some(value);
                }
                _ => {}
            }
        }
        if row.rbytes.is_none() && row.wbytes.is_none() && row.rios.is_none() && row.wios.is_none()
        {
            continue;
        }
        if rows.len() == limit {
            return None;
        }
        rows.push(row);
    }
    Some(rows)
}

fn key_value_lines(content: &str) -> impl Iterator<Item = (&str, i64)> {
    content.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        Some((fields.next()?, fields.next()?.parse().unwrap_or(0)))
    })
}

pub(super) fn parse_optional_max(content: &str) -> Option<i64> {
    let trimmed = content.trim();
    if trimmed == "max" {
        None
    } else {
        parse_i64(trimmed)
    }
}

pub(super) fn parse_i64(content: &str) -> Option<i64> {
    content.trim().parse().ok()
}

fn parse_device(device: &str) -> Option<(u32, u32)> {
    let (major, minor) = device.split_once(':')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_handles_unlimited() {
        assert_eq!(parse_cpu_max("max 100000\n"), (-1, 100_000));
        assert_eq!(parse_cpu_max("200000 100000\n"), (200_000, 100_000));
    }

    #[test]
    fn io_stat_parses_per_device_counters() {
        let rows = parse_io_stat("8:0 rbytes=1 wbytes=2 rios=3 wios=4 dbytes=9\n", 5, "/x");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].major, rows[0].minor), (8, 0));
        assert_eq!(rows[0].rbytes, Some(1));
        assert_eq!(rows[0].wios, Some(4));
    }

    #[test]
    fn io_stat_keeps_a_row_with_partial_counters() {
        let rows = parse_io_stat("8:0 rbytes=1 wbytes=2 rios=broken\n", 5, "/x");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rbytes, Some(1));
        assert_eq!(rows[0].wbytes, Some(2));
        assert_eq!(rows[0].rios, None);
        assert_eq!(rows[0].wios, None);
    }
}
