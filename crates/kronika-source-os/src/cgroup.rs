//! Parse and collect cgroup v2 metrics.

use std::collections::BTreeSet;
use std::io;

use crate::proc::pressure::{PsiRow, parse_pressure_at};
use crate::{ProcFs, SysFs};

mod model;
mod parse;
mod sections;
mod selected;

pub use selected::{
    AncestorContext, SelectedCgroup, charged_ancestor_devices, collect_ancestor_context,
    collect_ancestor_pressure, collect_ancestor_rows,
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

use parse::{
    parse_i64, parse_io_stat_bounded, parse_memory_events, parse_memory_stat_v2, parse_optional_max,
};

const CGROUP_ROOT: &str = "fs/cgroup";
const DEFAULT_CPU_PERIOD_USEC: i64 = 100_000;

/// Maximum distinct direct controller/path memberships accepted in one tick.
pub const MAX_CGROUP_CANDIDATES: usize = 512;
/// Maximum bytes across distinct direct controller/path memberships in one tick.
pub const MAX_CGROUP_PATH_BYTES: usize = 512 * 1024;
/// Maximum cgroup/device I/O rows accepted in one tick.
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

#[derive(Debug, Default)]
struct WorkloadCgroupPaths {
    unified: BTreeSet<String>,
    candidates: usize,
    path_bytes: usize,
}

impl WorkloadCgroupPaths {
    fn insert(&mut self, path: &str) -> io::Result<()> {
        let paths = &mut self.unified;
        if paths.contains(path) {
            return Ok(());
        }
        let path_len = path.len();
        paths.insert(path.to_owned());
        self.candidates = self.candidates.saturating_add(1);
        self.path_bytes = self.path_bytes.saturating_add(path_len);
        if self.candidates > MAX_CGROUP_CANDIDATES {
            return Err(io::Error::other(format!(
                "direct cgroup membership count exceeds {MAX_CGROUP_CANDIDATES}"
            )));
        }
        if self.path_bytes > MAX_CGROUP_PATH_BYTES {
            return Err(io::Error::other(format!(
                "direct cgroup membership paths exceed {MAX_CGROUP_PATH_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// Bounded, deduplicated direct cgroup paths observed while processes are read.
///
/// Raw `/proc/<pid>/cgroup` strings are parsed immediately and are not retained.
/// A hard candidate/path limit failure is remembered and returned by
/// [`collect`](Self::collect), so process collection can continue independently.
#[derive(Debug)]
pub struct WorkloadMemberships {
    unified_v2: bool,
    paths: WorkloadCgroupPaths,
    error: Option<io::Error>,
}

impl WorkloadMemberships {
    /// Create an empty accumulator for the cgroup hierarchy exposed by `sys`.
    #[must_use]
    pub fn new(sys: &SysFs) -> Self {
        Self {
            unified_v2: is_v2(sys),
            paths: WorkloadCgroupPaths::default(),
            error: None,
        }
    }

    /// Parse one process's direct controller memberships into the bounded set.
    pub fn observe(&mut self, content: &str) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.observe_inner(content) {
            self.error = Some(error);
        }
    }

    fn observe_inner(&mut self, content: &str) -> io::Result<()> {
        if self.unified_v2
            && let Some(path) = parse_unified_cgroup_path(content)
        {
            self.paths.insert(path)?;
        }
        Ok(())
    }

    /// Read metrics for the distinct direct paths observed so far.
    ///
    /// # Errors
    /// Returns the first hard candidate/path ceiling failure.
    pub fn collect(self, sys: &SysFs, ts: i64) -> io::Result<CgroupCollection> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(if self.unified_v2 {
            collect_v2_paths(sys, ts, self.paths.unified)
        } else {
            CgroupCollection::default()
        })
    }
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

/// Collect bounded metrics from already-read direct process memberships.
///
/// # Errors
/// Returns a hard candidate/path ceiling error.
pub fn collect_workload_memberships<I, S>(
    memberships: I,
    sys: &SysFs,
    ts: i64,
) -> io::Result<CgroupCollection>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut observed = WorkloadMemberships::new(sys);
    for content in memberships {
        observed.observe(content.as_ref());
    }
    observed.collect(sys, ts)
}

fn is_v2(sys: &SysFs) -> bool {
    sys.read(&rel("/", "cgroup.controllers")).is_ok()
        || sys.read(&rel("/", "memory.current")).is_ok()
        || sys.read(&rel("/", "io.stat")).is_ok()
}

fn collect_v2_paths(
    sys: &SysFs,
    ts: i64,
    paths: impl IntoIterator<Item = String>,
) -> CgroupCollection {
    let mut out = CgroupCollection::default();
    for path in paths {
        if let Some(cpu) = read_cpu_v2(sys, ts, &path) {
            out.cpu.push(cpu);
        }
        if let Some(memory) = read_memory_v2(sys, ts, &path) {
            out.memory.push(memory);
        }
        if let Some(pids) = read_pids_v2(sys, ts, &path) {
            out.pids.push(pids);
        }
        if !out.io_omitted
            && let Ok(content) = sys.read(&rel(&path, "io.stat"))
        {
            let remaining = MAX_CGROUP_IO_ROWS.saturating_sub(out.io.len());
            append_bounded_io(
                &mut out,
                parse_io_stat_bounded(&content, ts, &path, remaining),
            );
        }
    }
    out
}

fn append_bounded_io(out: &mut CgroupCollection, rows: Option<Vec<CgroupIoRow>>) {
    if let Some(rows) = rows {
        out.io.extend(rows);
    } else {
        out.io.clear();
        out.io_omitted = true;
    }
}

fn read_cpu_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupCpuRow> {
    let stat = sys.read(&rel(path, "cpu.stat")).ok()?;
    let mut row = parse_cpu_stat(&stat, ts, path);
    if let Ok(max) = sys.read(&rel(path, "cpu.max")) {
        let (quota, period) = parse_cpu_max(&max);
        row.quota_usec = quota;
        row.period_usec = period;
    }
    Some(row)
}

fn read_memory_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupMemoryRow> {
    let current = parse_i64(&sys.read(&rel(path, "memory.current")).ok()?)?;
    let mut row = CgroupMemoryRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: sys
            .read(&rel(path, "memory.max"))
            .ok()
            .and_then(|content| parse_optional_max(&content)),
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
    if let Ok(content) = sys.read(&rel(path, "memory.stat")) {
        parse_memory_stat_v2(&content, &mut row);
    }
    if let Ok(content) = sys.read(&rel(path, "memory.events")) {
        parse_memory_events(&content, &mut row);
    }
    Some(row)
}

fn read_pids_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupPidsRow> {
    let current = sys.read(&rel(path, "pids.current")).ok()?;
    let max = sys.read(&rel(path, "pids.max")).ok()?;
    let (current, max) = parse_pids_values(&current, &max)?;
    Some(CgroupPidsRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max,
    })
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

fn rel(path: &str, file: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("{CGROUP_ROOT}/{file}")
    } else {
        format!("{CGROUP_ROOT}/{path}/{file}")
    }
}

#[cfg(test)]
mod tests;
