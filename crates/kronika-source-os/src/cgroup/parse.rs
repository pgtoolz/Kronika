//! Parsers for cgroup controller files.

use std::collections::BTreeMap;

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

pub(super) fn parse_cpuacct_stat(content: &str, clock_ticks_per_sec: i64, row: &mut CgroupCpuRow) {
    for (key, value) in key_value_lines(content) {
        let usec = ticks_to_usec(value, clock_ticks_per_sec);
        match key {
            "user" => row.user_usec = usec,
            "system" => row.system_usec = usec,
            _ => {}
        }
    }
}

pub(super) fn parse_v1_cpu_stat(content: &str, row: &mut CgroupCpuRow) {
    for (key, value) in key_value_lines(content) {
        match key {
            "nr_throttled" => row.nr_throttled = value,
            "throttled_time" => row.throttled_usec = value / 1000,
            _ => {}
        }
    }
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

pub(super) fn parse_memory_stat_v1(content: &str, row: &mut CgroupMemoryRow) {
    let mut kernel_stack = 0_i64;
    for (key, value) in key_value_lines(content) {
        match key {
            "rss" | "total_rss" => row.anon = value,
            "cache" | "total_cache" => row.file = value,
            "slab" | "total_slab" => row.slab = value,
            "kernel_stack" | "total_kernel_stack" => kernel_stack = value,
            _ => {}
        }
    }
    row.kernel = row.slab.saturating_add(kernel_stack);
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

/// Parse cgroup v1 blkio service byte/op files.
#[must_use]
pub fn parse_blkio_service_stats(
    bytes_content: &str,
    ops_content: &str,
    ts: i64,
    cgroup_path: &str,
) -> Vec<CgroupIoRow> {
    parse_blkio_service_stats_bounded(bytes_content, ops_content, ts, cgroup_path, usize::MAX)
        .unwrap_or_default()
}

pub(super) fn parse_blkio_service_stats_bounded(
    bytes_content: &str,
    ops_content: &str,
    ts: i64,
    cgroup_path: &str,
    limit: usize,
) -> Option<Vec<CgroupIoRow>> {
    let mut rows = BTreeMap::new();
    if !parse_blkio_service_file(bytes_content, ts, cgroup_path, true, limit, &mut rows)
        || !parse_blkio_service_file(ops_content, ts, cgroup_path, false, limit, &mut rows)
    {
        return None;
    }
    Some(rows.into_values().collect())
}

fn parse_blkio_service_file(
    content: &str,
    ts: i64,
    cgroup_path: &str,
    bytes: bool,
    limit: usize,
    rows: &mut BTreeMap<(u32, u32), CgroupIoRow>,
) -> bool {
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let Some(device) = fields.next() else {
            continue;
        };
        let Some(op) = fields.next() else {
            continue;
        };
        if op == "Total" {
            continue;
        }
        if !matches!(op, "Read" | "Write") {
            continue;
        }
        let Some(value) = fields.next().and_then(|value| value.parse().ok()) else {
            continue;
        };
        let Some((major, minor)) = parse_device(device) else {
            continue;
        };
        if !rows.contains_key(&(major, minor)) && rows.len() == limit {
            return false;
        }
        let row = rows.entry((major, minor)).or_insert_with(|| CgroupIoRow {
            ts,
            cgroup_path: cgroup_path.to_owned(),
            major,
            minor,
            rbytes: None,
            wbytes: None,
            rios: None,
            wios: None,
        });
        match (op, bytes) {
            ("Read", true) => {
                row.rbytes = Some(value);
            }
            ("Write", true) => {
                row.wbytes = Some(value);
            }
            ("Read", false) => {
                row.rios = Some(value);
            }
            ("Write", false) => {
                row.wios = Some(value);
            }
            _ => {}
        }
    }
    true
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

pub(super) fn parse_v1_memory_limit(content: &str) -> Option<i64> {
    let value = parse_i64(content)?;
    (value < i64::MAX / 2).then_some(value)
}

pub(super) fn parse_i64(content: &str) -> Option<i64> {
    content.trim().parse().ok()
}

fn parse_device(device: &str) -> Option<(u32, u32)> {
    let (major, minor) = device.split_once(':')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn ticks_to_usec(ticks: i64, hz: i64) -> i64 {
    if hz <= 0 {
        return 0;
    }
    i64::try_from(i128::from(ticks).saturating_mul(1_000_000) / i128::from(hz)).unwrap_or(i64::MAX)
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
    fn cpuacct_stat_uses_hz() {
        let mut row = parse_cpu_stat("", 1, "/");
        parse_cpuacct_stat("user 250\nsystem 125\n", 250, &mut row);
        assert_eq!(row.user_usec, 1_000_000);
        assert_eq!(row.system_usec, 500_000);
    }

    #[test]
    fn memory_v1_maps_total_fields_and_kernel_stack() {
        let mut row = CgroupMemoryRow {
            ts: 1,
            cgroup_path: "/x".to_owned(),
            current: 0,
            max: None,
            anon: 0,
            file: 0,
            kernel: 0,
            slab: 0,
            low_events: 0,
            high_events: 0,
            max_events: 0,
            oom_events: 0,
            oom_kill: 0,
        };
        parse_memory_stat_v1(
            "total_rss 10\ntotal_cache 20\ntotal_slab 3\ntotal_kernel_stack 4\n",
            &mut row,
        );
        assert_eq!(row.anon, 10);
        assert_eq!(row.file, 20);
        assert_eq!(row.slab, 3);
        assert_eq!(row.kernel, 7);
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

    #[test]
    fn blkio_parser_skips_total_and_merges_ops() {
        let rows = parse_blkio_service_stats(
            "8:0 Read 4096\n8:0 Write 8192\n8:0 Total 12288\n",
            "8:0 Read 4\n8:0 Write 8\n8:0 Total 12\n",
            1,
            "/x",
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rbytes, Some(4096));
        assert_eq!(rows[0].wbytes, Some(8192));
        assert_eq!(rows[0].rios, Some(4));
        assert_eq!(rows[0].wios, Some(8));
    }

    #[test]
    fn blkio_parser_keeps_devices_and_files_independently() {
        let rows = parse_blkio_service_stats(
            "8:0 Read 4096\n8:1 Write 8192\n",
            "8:0 Read 4\n8:2 Write 8\n",
            1,
            "/x",
        );
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].rbytes, rows[0].rios), (Some(4096), Some(4)));
        assert_eq!((rows[1].wbytes, rows[1].wios), (Some(8192), None));
        assert_eq!((rows[2].wbytes, rows[2].wios), (None, Some(8)));
    }
}
