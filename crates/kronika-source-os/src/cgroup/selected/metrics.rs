//! Read controller metrics only while the selected directory identity remains current.

use super::{AncestorContext, SelectedCgroup};
use crate::SysFs;
use crate::cgroup::parse::parse_i64;
use crate::cgroup::{
    self, CgroupCollection, CpuQuota, MAX_CGROUP_IO_ROWS, MemoryLimit, PsiRow, hierarchy_paths,
    parse_cpu_max_strict, parse_io_stat_bounded, parse_pressure_at,
};
use crate::proc::stat::ParseError;

// These are observed limits of the selected object, not assertions about
// unseen constraints or PostgreSQL's resource scope.
pub(super) fn observed_cpu_limit(sys: &SysFs, group: &SelectedCgroup) -> Option<(i64, i64)> {
    if !group.is_current(sys) {
        return None;
    }
    let mut finite = None;
    let mut unlimited_period = None;
    for path in hierarchy_paths(&group.path)? {
        let quota = sys
            .read(&group.relative(&path, "cpu.max"))
            .ok()
            .and_then(|value| parse_cpu_max_strict(&value));
        if let Some(quota) = quota {
            if let CpuQuota::Unlimited { period_usec } = quota {
                unlimited_period = Some(period_usec);
            }
            cgroup::update_effective_cpu(&mut finite, quota);
        }
    }
    if !group.is_current(sys) {
        return None;
    }
    finite.or_else(|| unlimited_period.map(|period| (-1, period)))
}

pub(super) fn effective_memory(sys: &SysFs, group: &SelectedCgroup) -> Option<i64> {
    if !group.is_current(sys) {
        return None;
    }
    let finite = |limit| match limit {
        MemoryLimit::Limited(value) => Some(value),
        MemoryLimit::Unlimited => None,
    };
    let limit = hierarchy_paths(&group.path)?
        .into_iter()
        .filter_map(|path| {
            sys.read(&group.relative(&path, "memory.max"))
                .ok()
                .and_then(|value| cgroup::parse_memory_max_strict(&value))
                .and_then(finite)
        })
        .min();
    if !group.is_current(sys) {
        return None;
    }
    limit
}

/// Read one selected aggregate per controller, omitting incomplete non-null rows.
#[must_use]
pub fn collect_ancestor_rows(sys: &SysFs, selected: &AncestorContext, ts: i64) -> CgroupCollection {
    let mut out = CgroupCollection::default();
    if let Some(group) = &selected.group
        && group.is_current(sys)
        && let Some(row) = read_cpu(sys, group, ts)
        && group.is_current(sys)
    {
        out.ancestor_cpu.push(row);
    }
    if let Some(group) = &selected.group
        && group.is_current(sys)
        && let Some(row) = read_memory(sys, group, ts)
        && group.is_current(sys)
    {
        out.ancestor_memory.push(row);
    }
    if let Some(group) = &selected.group
        && group.is_current(sys)
        && let Some(content) = group.read(sys, "io.stat")
    {
        match parse_io_stat_bounded(&content, ts, &group.path, MAX_CGROUP_IO_ROWS) {
            Some(rows) => out.io = rows,
            None => out.io_omitted = true,
        }
    }

    if selected
        .group
        .as_ref()
        .is_some_and(|group| !group.is_current(sys))
    {
        out.io.clear();
    }
    if let Some(group) = &selected.group
        && group.is_current(sys)
        && let Some((current, max)) = group
            .read(sys, "pids.current")
            .zip(group.read(sys, "pids.max"))
            .and_then(|(current, max)| cgroup::parse_pids_values(&current, &max))
        && group.is_current(sys)
    {
        out.pids.push(cgroup::CgroupPidsRow {
            ts,
            cgroup_path: group.path.clone(),
            current,
            max,
        });
    }

    out
}

fn read_cpu(sys: &SysFs, group: &SelectedCgroup, ts: i64) -> Option<cgroup::AncestorCpuRow> {
    let stat = group.read(sys, "cpu.stat");
    let value = |key| {
        stat.as_deref()
            .and_then(|text| cgroup::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let usage = value("usage_usec")?;
    let user = value("user_usec")?;
    let system = value("system_usec")?;
    let throttled = value("throttled_usec");
    let quota = group
        .read(sys, "cpu.max")
        .and_then(|value| parse_cpu_max_strict(&value));
    let pair = quota.map(CpuQuota::pair);

    Some(cgroup::AncestorCpuRow {
        ts,
        cgroup_path: group.path.clone(),
        usage_usec: usage,
        user_usec: user,
        system_usec: system,
        throttled_usec: throttled,
        nr_throttled: value("nr_throttled"),
        quota_usec: pair.map(|pair| pair.0),
        period_usec: pair.map(|pair| pair.1),
    })
}

fn read_memory(sys: &SysFs, group: &SelectedCgroup, ts: i64) -> Option<cgroup::AncestorMemoryRow> {
    let current = parse_i64(&group.read(sys, "memory.current")?)?;
    if current < 0 {
        return None;
    }
    let limit = group
        .read(sys, "memory.max")
        .and_then(|value| cgroup::parse_memory_max_strict(&value));
    let stat = group.read(sys, "memory.stat");
    let events = group.read(sys, "memory.events");
    let stat_value = |key| {
        stat.as_deref()
            .and_then(|text| cgroup::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let event = |key| {
        events
            .as_deref()
            .and_then(|text| cgroup::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let (anon, file, kernel, slab) = (
        stat_value("anon"),
        stat_value("file"),
        stat_value("kernel"),
        stat_value("slab"),
    );
    Some(cgroup::AncestorMemoryRow {
        ts,
        cgroup_path: group.path.clone(),
        current,
        max: match limit {
            Some(MemoryLimit::Limited(value)) => Some(value),
            _ => None,
        },
        max_unlimited: limit.map(|limit| matches!(limit, MemoryLimit::Unlimited)),
        anon,
        file,
        kernel,
        slab,
        low_events: event("low"),
        high_events: event("high"),
        max_events: event("max"),
        oom_events: event("oom"),
        oom_kill: event("oom_kill"),
    })
}

/// Read PSI only from the selected v2 ancestor.
///
/// # Errors
/// Returns membership/selection or present pressure parsing errors.
pub fn collect_ancestor_pressure(
    sys: &SysFs,
    selected: &AncestorContext,
    ts: i64,
) -> Result<Vec<PsiRow>, ParseError> {
    let Some(group) = selected.group.as_ref() else {
        return Ok(Vec::new());
    };
    if !group.is_current(sys) {
        return Ok(Vec::new());
    }
    let cpu = group.read(sys, "cpu.pressure");
    let memory = group.read(sys, "memory.pressure");
    let io = group.read(sys, "io.pressure");
    if !group.is_current(sys) {
        return Ok(Vec::new());
    }
    parse_pressure_at(
        cpu.as_deref(),
        memory.as_deref(),
        io.as_deref(),
        ts,
        &group.path,
        ["cpu.pressure", "memory.pressure", "io.pressure"],
        true,
    )
}

/// Device IDs charged to the selected v2 ancestor, without a child fallback.
#[must_use]
pub fn charged_ancestor_devices(sys: &SysFs, selected: &AncestorContext) -> Vec<(i32, i32)> {
    let Some(group) = selected.group.as_ref() else {
        return Vec::new();
    };
    if !group.is_current(sys) {
        return Vec::new();
    }
    let devices = cgroup::discovery::charged_devices(sys, group).unwrap_or_default();
    if group.is_current(sys) {
        devices
    } else {
        Vec::new()
    }
}
