//! Parse and collect cgroup v2 metrics.

use crate::proc::pressure::{PsiRow, parse_pressure_at};
use crate::{ProcFs, SysFs};

pub mod discovery;
mod model;
mod parse;
mod sections;
mod selected;

pub use selected::{
    AncestorContext, SelectedCgroup, charged_ancestor_devices, collect_ancestor_context,
    collect_ancestor_pressure, collect_ancestor_rows, select_ancestor_context,
};

pub use model::{
    AncestorCpuRow, AncestorMemoryRow, CgroupCollection, CgroupContextRow, CgroupCpuRow,
    CgroupIoRow, CgroupMemoryRow, CgroupPidsRow,
};
pub use parse::{parse_cpu_max, parse_cpu_stat, parse_io_stat};
pub use sections::{
    to_ancestor_context_section, to_ancestor_cpu_section, to_ancestor_io_section,
    to_ancestor_memory_section, to_cpu_section, to_io_section, to_memory_section, to_pids_section,
};

use parse::{parse_i64, parse_io_stat_bounded};

const DEFAULT_CPU_PERIOD_USEC: i64 = 100_000;

/// Maximum I/O rows in the selected-primary compatibility reader.
pub const MAX_CGROUP_IO_ROWS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CpuQuota {
    Unlimited { period_usec: i64 },
    Limited { quota_usec: i64, period_usec: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryLimit {
    Unlimited,
    Limited(i64),
}

pub(crate) fn parse_unified_cgroup_path(content: &str) -> Option<&str> {
    let mut path = None;
    let mut unified_seen = false;
    for line in content.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(raw_path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if controllers.is_empty() && hierarchy == "0" {
            path = if unified_seen {
                None
            } else {
                normalize_self_cgroup_path(raw_path)
            };
            unified_seen = true;
        }
    }
    path
}

fn normalize_self_cgroup_path(path: &str) -> Option<&str> {
    let path = path.trim();
    if !path.starts_with('/')
        || path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
    {
        return None;
    }
    let normalized = path.trim_end_matches('/');
    Some(if normalized.is_empty() {
        "/"
    } else {
        normalized
    })
}

fn parse_cpuset_count(content: &str) -> Option<i64> {
    let mut count = 0_u64;
    let mut previous_end = None;
    for part in content.trim().split(',') {
        if part.is_empty() {
            return None;
        }
        let (start, end) = if let Some((start, end)) = part.split_once('-') {
            if end.contains('-') {
                return None;
            }
            (start.parse::<u32>().ok()?, end.parse::<u32>().ok()?)
        } else {
            let cpu = part.parse::<u32>().ok()?;
            (cpu, cpu)
        };
        if start > end || previous_end.is_some_and(|previous| start <= previous) {
            return None;
        }
        count = count.checked_add(u64::from(end - start) + 1)?;
        previous_end = Some(end);
    }
    (count != 0).then(|| i64::try_from(count).ok()).flatten()
}

fn update_effective_cpu(effective: &mut Option<(i64, i64)>, candidate: CpuQuota) {
    let CpuQuota::Limited {
        quota_usec,
        period_usec,
    } = candidate
    else {
        return;
    };
    let replace = match *effective {
        Some((current_quota, current_period)) => {
            i128::from(quota_usec) * i128::from(current_period)
                < i128::from(current_quota) * i128::from(period_usec)
        }
        None => true,
    };
    if replace {
        *effective = Some((quota_usec, period_usec));
    }
}

fn parse_cpu_max_strict(content: &str) -> Option<CpuQuota> {
    let mut fields = content.split_whitespace();
    let quota = fields.next()?;
    let period_usec = fields.next()?.parse::<i64>().ok()?;
    if fields.next().is_some() || period_usec <= 0 {
        return None;
    }
    if quota == "max" {
        Some(CpuQuota::Unlimited { period_usec })
    } else {
        let quota_usec = quota.parse::<i64>().ok()?;
        (quota_usec > 0).then_some(CpuQuota::Limited {
            quota_usec,
            period_usec,
        })
    }
}

fn parse_memory_max_strict(content: &str) -> Option<MemoryLimit> {
    let mut fields = content.split_whitespace();
    let value = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    if value == "max" {
        Some(MemoryLimit::Unlimited)
    } else {
        let limit = value.parse::<i64>().ok()?;
        (limit >= 0).then_some(MemoryLimit::Limited(limit))
    }
}

fn parse_exact_stat_value(content: &str, wanted: &str) -> Option<i64> {
    let mut found = None;
    for line in content.lines() {
        let mut fields = line.split_whitespace();
        if fields.next()? != wanted {
            continue;
        }
        let value = fields.next()?.parse::<i64>().ok()?;
        if fields.next().is_some() || found.replace(value).is_some() {
            return None;
        }
    }
    found
}

fn hierarchy_paths(path: &str) -> Option<Vec<String>> {
    if normalize_self_cgroup_path(path) != Some(path) {
        return None;
    }
    let mut paths = vec!["/".to_owned()];
    if path == "/" {
        return Some(paths);
    }
    let mut current = String::new();
    for component in path.trim_start_matches('/').split('/') {
        current.push('/');
        current.push_str(component);
        paths.push(current.clone());
    }
    Some(paths)
}

fn parse_pids_values(current: &str, max: &str) -> Option<(i64, Option<i64>)> {
    let current = parse_i64(current).filter(|value| *value >= 0)?;
    let max = if max == "max" {
        None
    } else {
        Some(parse_i64(max).filter(|value| *value >= 0)?)
    };
    Some((current, max))
}

#[cfg(test)]
mod tests;
