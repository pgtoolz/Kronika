//! Parse and collect cgroup v2 and selected cgroup v1 metrics.

use std::collections::{BTreeSet, VecDeque};
use std::io;

use crate::proc::pressure::{PsiRow, parse_pressure_at};
use crate::proc::stat::ParseError;
use crate::{ProcFs, SysFs};

mod model;
mod parse;
mod sections;

pub use model::{
    CgroupCollection, CgroupContextRow, CgroupCpuRow, CgroupIoRow, CgroupMemoryRow, CgroupPidsRow,
};
pub use parse::{parse_blkio_service_stats, parse_cpu_max, parse_cpu_stat, parse_io_stat};
pub use sections::{
    to_context_section, to_cpu_section, to_io_section, to_memory_section, to_pids_section,
};

use parse::{
    parse_blkio_service_stats_bounded, parse_cpuacct_stat, parse_i64, parse_io_stat_bounded,
    parse_memory_events, parse_memory_stat_v1, parse_memory_stat_v2, parse_optional_max,
    parse_v1_cpu_stat, parse_v1_memory_limit,
};

const CGROUP_ROOT: &str = "fs/cgroup";
const DEFAULT_CPU_PERIOD_USEC: i64 = 100_000;

/// Maximum distinct direct controller/path memberships accepted in one tick.
pub const MAX_CGROUP_CANDIDATES: usize = 512;
/// Maximum bytes across distinct direct controller/path memberships in one tick.
pub const MAX_CGROUP_PATH_BYTES: usize = 512 * 1024;
/// Maximum cgroup/device I/O rows accepted in one tick.
pub const MAX_CGROUP_IO_ROWS: usize = 1024;

const CPU_V1_DIRS: &[&str] = &["cpu,cpuacct", "cpuacct,cpu", "cpu", "cpuacct", ""];
const CPU_QUOTA_V1_DIRS: &[&str] = &["cpu,cpuacct", "cpuacct,cpu", "cpu", ""];
const MEMORY_V1_DIRS: &[&str] = &["memory", ""];
const PIDS_V1_DIRS: &[&str] = &["pids", ""];
const BLKIO_V1_DIRS: &[&str] = &["blkio", ""];
const CPUSET_V1_DIRS: &[&str] = &["cpuset", ""];

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

#[derive(Debug, Default, PartialEq, Eq)]
struct SelfCgroupPaths<'a> {
    unified: Option<&'a str>,
    cpu: Option<&'a str>,
    cpuacct: Option<&'a str>,
    memory: Option<&'a str>,
    io: Option<&'a str>,
    pids: Option<&'a str>,
    cpuset: Option<&'a str>,
}

#[derive(Debug, Default)]
struct WorkloadCgroupPaths {
    unified: BTreeSet<String>,
    cpu: BTreeSet<String>,
    memory: BTreeSet<String>,
    io: BTreeSet<String>,
    pids: BTreeSet<String>,
    candidates: usize,
    path_bytes: usize,
}

impl WorkloadCgroupPaths {
    fn insert(&mut self, controller: CgroupController, path: &str) -> io::Result<()> {
        let paths = match controller {
            CgroupController::Unified => &mut self.unified,
            CgroupController::Cpu => &mut self.cpu,
            CgroupController::Memory => &mut self.memory,
            CgroupController::Io => &mut self.io,
            CgroupController::Pids => &mut self.pids,
        };
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

#[derive(Debug, Clone, Copy)]
enum CgroupController {
    Unified,
    Cpu,
    Memory,
    Io,
    Pids,
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
        let parsed = parse_self_cgroup(content);
        if self.unified_v2 {
            if let Some(path) = parsed.unified {
                self.paths.insert(CgroupController::Unified, path)?;
            }
            return Ok(());
        }
        if let Some(path) = matching_cpu_path(parsed.cpu, parsed.cpuacct) {
            self.paths.insert(CgroupController::Cpu, path)?;
        }
        if let Some(path) = parsed.memory {
            self.paths.insert(CgroupController::Memory, path)?;
        }
        if let Some(path) = parsed.io {
            self.paths.insert(CgroupController::Io, path)?;
        }
        if let Some(path) = parsed.pids {
            self.paths.insert(CgroupController::Pids, path)?;
        }
        Ok(())
    }

    /// Read metrics for the distinct direct paths observed so far.
    ///
    /// # Errors
    /// Returns the first hard candidate/path ceiling failure.
    pub fn collect(
        self,
        sys: &SysFs,
        ts: i64,
        clock_ticks_per_sec: i64,
    ) -> io::Result<CgroupCollection> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(if self.unified_v2 {
            collect_v2_paths(sys, ts, self.paths.unified)
        } else {
            collect_v1_paths(sys, ts, clock_ticks_per_sec, self.paths)
        })
    }
}

/// Read PSI from the collector process's exact unified cgroup v2 membership.
///
/// Cgroup v1 and systems without a unified hierarchy return no rows. Missing
/// resource files omit only those resources. Host pressure is never read here.
///
/// # Errors
/// Returns an error when cgroup v2 is present but the exact membership cannot
/// be read, is invalid or ambiguous, a present pressure file cannot be read,
/// or a present pressure value cannot be parsed.
pub fn collect_pressure(procfs: &ProcFs, sys: &SysFs, ts: i64) -> Result<Vec<PsiRow>, ParseError> {
    if !is_v2(sys) {
        return Ok(Vec::new());
    }

    let membership = procfs
        .read_raw("self/cgroup")
        .map_err(|err| ParseError(format!("self/cgroup: {err}")))?;
    let path = parse_self_cgroup(&membership).unified.ok_or_else(|| {
        ParseError("self/cgroup: no single valid unified cgroup membership".to_owned())
    })?;
    let cpu = read_optional_pressure(sys, path, "cpu.pressure")?;
    let memory = read_optional_pressure(sys, path, "memory.pressure")?;
    let io = read_optional_pressure(sys, path, "io.pressure")?;

    parse_pressure_at(
        cpu.as_deref(),
        memory.as_deref(),
        io.as_deref(),
        ts,
        path,
        ["cpu.pressure", "memory.pressure", "io.pressure"],
    )
    .map_err(|err| ParseError(format!("cgroup v2: {err}")))
}

/// Block devices charged by the collector's own cgroup v2 `io.stat`.
///
/// Cgroup v1 returns no devices.
///
/// # Errors
/// Returns the membership or `io.stat` read error, or an invalid membership.
pub fn charged_devices(procfs: &ProcFs, sys: &SysFs) -> io::Result<Vec<(i32, i32)>> {
    if !is_v2(sys) {
        return Ok(Vec::new());
    }
    let membership = procfs
        .read_raw("self/cgroup")
        .map_err(|err| io::Error::new(err.kind(), format!("self/cgroup: {err}")))?;
    let path = parse_self_cgroup(&membership).unified.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "self/cgroup: no single valid unified cgroup membership",
        )
    })?;
    let relative = rel(path, "io.stat");
    let content = sys
        .read(&relative)
        .map_err(|err| io::Error::new(err.kind(), format!("{relative}: {err}")))?;
    Ok(parse_io_stat(&content, 0, path)
        .iter()
        .filter_map(|row| {
            Some((
                i32::try_from(row.major).ok()?,
                i32::try_from(row.minor).ok()?,
            ))
        })
        .collect())
}

fn read_optional_pressure(
    sys: &SysFs,
    path: &str,
    file: &str,
) -> Result<Option<String>, ParseError> {
    let relative = rel(path, file);
    match sys.read(&relative) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ParseError(format!("{relative}: {err}"))),
    }
}

/// Collect the process's exact cgroup paths, effective cpuset, and capacity.
///
/// Cpuset comes only from the exact effective file. Hierarchical capacity stays
/// absent unless the applicable controller path can be validated coherently.
///
/// # Errors
/// Returns the procfs read error when `self/cgroup` is unavailable.
pub fn collect_context(procfs: &ProcFs, sys: &SysFs, ts: i64) -> io::Result<CgroupContextRow> {
    let content = procfs.read_raw("self/cgroup")?;
    let parsed = parse_self_cgroup(&content);
    let unified_v2 = is_v2(sys);
    let has_unified = parsed.unified.is_some();
    let has_v1 = parsed.cpu.is_some()
        || parsed.cpuacct.is_some()
        || parsed.memory.is_some()
        || parsed.io.is_some()
        || parsed.cpuset.is_some();

    if unified_v2 && has_unified {
        let path = parsed.unified.map(str::to_owned);
        let cpuset_cpus = path.as_deref().and_then(|path| {
            sys.read(&rel(path, "cpuset.cpus.effective"))
                .ok()
                .and_then(|content| parse_cpuset_count(&content))
        });
        let cpu_path = path.clone().filter(|path| usable_cpu_v2(sys, path));
        let memory_path = path.clone().filter(|path| usable_memory_v2(sys, path));
        let (effective_cpu_quota_usec, effective_cpu_period_usec) = cpu_path
            .as_deref()
            .and_then(|path| effective_cpu_v2(sys, path))
            .map_or((None, None), |(quota, period)| (Some(quota), Some(period)));
        let effective_memory_max = memory_path
            .as_deref()
            .and_then(|path| effective_memory_v2(sys, path));
        return Ok(CgroupContextRow {
            ts,
            cgroup_version: 2,
            cpu_path,
            memory_path,
            io_path: path.filter(|path| usable_io_v2(sys, path)),
            cpuset_cpus,
            effective_cpu_quota_usec,
            effective_cpu_period_usec,
            effective_memory_max,
        });
    }

    if !unified_v2 && has_v1 {
        let cpu_path = matching_cpu_path(parsed.cpu, parsed.cpuacct)
            .map(str::to_owned)
            .filter(|path| usable_cpu_v1(sys, path));
        let cpuset_cpus = parsed.cpuset.and_then(|path| {
            let root = bind_v1_controller_root(sys, CPUSET_V1_DIRS, path, "cpuset.effective_cpus")?;
            read_bound_v1(sys, &root, path, "cpuset.effective_cpus")
                .and_then(|content| parse_cpuset_count(&content))
        });
        let memory_path = parsed
            .memory
            .map(str::to_owned)
            .filter(|path| usable_memory_v1(sys, path));
        let (effective_cpu_quota_usec, effective_cpu_period_usec) = cpu_path
            .as_deref()
            .and_then(|path| effective_cpu_v1(sys, path))
            .map_or((None, None), |(quota, period)| (Some(quota), Some(period)));
        let effective_memory_max = memory_path
            .as_deref()
            .and_then(|path| effective_memory_v1(sys, path));
        return Ok(CgroupContextRow {
            ts,
            cgroup_version: 1,
            cpu_path,
            memory_path,
            io_path: parsed
                .io
                .map(str::to_owned)
                .filter(|path| usable_io_v1(sys, path)),
            cpuset_cpus,
            effective_cpu_quota_usec,
            effective_cpu_period_usec,
            effective_memory_max,
        });
    }

    Ok(CgroupContextRow {
        ts,
        ..CgroupContextRow::default()
    })
}

fn matching_cpu_path<'a>(cpu: Option<&'a str>, cpuacct: Option<&'a str>) -> Option<&'a str> {
    match (cpu, cpuacct) {
        (Some(cpu), Some(cpuacct)) if cpu == cpuacct => Some(cpu),
        _ => None,
    }
}

fn usable_cpu_v2(sys: &SysFs, path: &str) -> bool {
    sys.read(&rel(path, "cpu.stat")).is_ok_and(|content| {
        has_numeric_keys(&content, &["usage_usec", "user_usec", "system_usec"])
    })
}

fn usable_cpu_v1(sys: &SysFs, path: &str) -> bool {
    read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.usage")
        .is_some_and(|content| parse_i64(&content).is_some())
        && read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.stat")
            .is_some_and(|content| has_numeric_keys(&content, &["user", "system"]))
}

fn usable_memory_v2(sys: &SysFs, path: &str) -> bool {
    sys.read(&rel(path, "memory.current"))
        .is_ok_and(|content| parse_i64(&content).is_some())
        && sys
            .read(&rel(path, "memory.stat"))
            .is_ok_and(|content| has_numeric_keys(&content, &["anon", "file", "kernel", "slab"]))
}

fn usable_memory_v1(sys: &SysFs, path: &str) -> bool {
    read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.usage_in_bytes")
        .is_some_and(|content| parse_i64(&content).is_some())
        && read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.stat").is_some_and(|content| {
            let keys = numeric_keys(&content);
            has_any_key(&keys, &["rss", "total_rss"])
                && has_any_key(&keys, &["cache", "total_cache"])
                && has_any_key(&keys, &["slab", "total_slab"])
                && has_any_key(&keys, &["kernel_stack", "total_kernel_stack"])
        })
}

fn usable_io_v2(sys: &SysFs, path: &str) -> bool {
    sys.read(&rel(path, "io.stat")).is_ok_and(|content| {
        parse_io_stat_bounded(&content, 0, path, MAX_CGROUP_IO_ROWS)
            .is_some_and(|rows| !rows.is_empty())
    })
}

fn usable_io_v1(sys: &SysFs, path: &str) -> bool {
    let bytes = read_first_v1(sys, BLKIO_V1_DIRS, path, "blkio.throttle.io_service_bytes")
        .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, path, "blkio.io_service_bytes"));
    let operations = read_first_v1(sys, BLKIO_V1_DIRS, path, "blkio.throttle.io_serviced")
        .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, path, "blkio.io_serviced"));
    parse_blkio_service_stats_bounded(
        bytes.as_deref().unwrap_or_default(),
        operations.as_deref().unwrap_or_default(),
        0,
        path,
        MAX_CGROUP_IO_ROWS,
    )
    .is_some_and(|rows| !rows.is_empty())
}

fn numeric_keys(content: &str) -> BTreeSet<&str> {
    content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let key = fields.next()?;
            fields.next()?.parse::<i64>().ok().map(|_value| key)
        })
        .collect()
}

fn has_numeric_keys(content: &str, required: &[&str]) -> bool {
    let keys = numeric_keys(content);
    required.iter().all(|key| keys.contains(key))
}

fn has_any_key(keys: &BTreeSet<&str>, candidates: &[&str]) -> bool {
    candidates.iter().any(|key| keys.contains(key))
}

fn parse_self_cgroup(content: &str) -> SelfCgroupPaths<'_> {
    let mut paths = SelfCgroupPaths::default();
    let mut unified_seen = false;
    for line in content.lines() {
        let mut fields = line.splitn(3, ':');
        let (Some(hierarchy), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if controllers.is_empty() {
            if hierarchy == "0" {
                paths.unified = if unified_seen {
                    None
                } else {
                    normalize_self_cgroup_path(path)
                };
                unified_seen = true;
            }
            continue;
        }
        let Some(path) = normalize_self_cgroup_path(path) else {
            continue;
        };
        for controller in controllers.split(',') {
            match controller {
                "cpu" => set_exact_path(&mut paths.cpu, path),
                "cpuacct" => set_exact_path(&mut paths.cpuacct, path),
                "memory" => set_exact_path(&mut paths.memory, path),
                "blkio" => set_exact_path(&mut paths.io, path),
                "pids" => set_exact_path(&mut paths.pids, path),
                "cpuset" => set_exact_path(&mut paths.cpuset, path),
                _ => {}
            }
        }
    }
    paths
}

fn set_exact_path<'a>(stored: &mut Option<&'a str>, path: &'a str) {
    if stored.is_none() {
        *stored = Some(path);
    } else if *stored != Some(path) {
        *stored = None;
    }
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

fn effective_cpu_v2(sys: &SysFs, path: &str) -> Option<(i64, i64)> {
    let mut effective = None;
    let mut leaf_period = None;
    for (index, ancestor) in hierarchy_paths(path)?.into_iter().enumerate() {
        let content = match sys.read(&rel(&ancestor, "cpu.max")) {
            Ok(content) => content,
            Err(err) if index == 0 && path != "/" && err.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            Err(_err) => return None,
        };
        let value = parse_cpu_max_strict(&content)?;
        leaf_period = Some(match value {
            CpuQuota::Unlimited { period_usec } | CpuQuota::Limited { period_usec, .. } => {
                period_usec
            }
        });
        update_effective_cpu(&mut effective, value);
    }
    effective.or_else(|| leaf_period.map(|period| (-1, period)))
}

fn effective_cpu_v1(sys: &SysFs, path: &str) -> Option<(i64, i64)> {
    let root = bind_v1_controller_root(sys, CPU_QUOTA_V1_DIRS, path, "cpu.cfs_quota_us")?;
    let mut effective = None;
    let mut leaf_period = None;
    for ancestor in hierarchy_paths(path)? {
        let quota = read_bound_v1(sys, &root, &ancestor, "cpu.cfs_quota_us")?;
        let period = read_bound_v1(sys, &root, &ancestor, "cpu.cfs_period_us")?;
        let value = parse_cpu_v1_quota_strict(&quota, &period)?;
        leaf_period = Some(match value {
            CpuQuota::Unlimited { period_usec } | CpuQuota::Limited { period_usec, .. } => {
                period_usec
            }
        });
        update_effective_cpu(&mut effective, value);
    }
    effective.or_else(|| leaf_period.map(|period| (-1, period)))
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

fn parse_cpu_v1_quota_strict(quota: &str, period: &str) -> Option<CpuQuota> {
    let quota_usec = quota.parse::<i64>().ok()?;
    let period_usec = period.parse::<i64>().ok()?;
    if period_usec <= 0 {
        return None;
    }
    match quota_usec {
        -1 => Some(CpuQuota::Unlimited { period_usec }),
        1.. => Some(CpuQuota::Limited {
            quota_usec,
            period_usec,
        }),
        _ => None,
    }
}

fn effective_memory_v2(sys: &SysFs, path: &str) -> Option<i64> {
    let mut effective = None;
    for (index, ancestor) in hierarchy_paths(path)?.into_iter().enumerate() {
        let content = match sys.read(&rel(&ancestor, "memory.max")) {
            Ok(content) => content,
            Err(err) if index == 0 && path != "/" && err.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            Err(_err) => return None,
        };
        if let MemoryLimit::Limited(limit) = parse_memory_max_strict(&content)? {
            effective = Some(effective.map_or(limit, |current: i64| current.min(limit)));
        }
    }
    effective
}

fn effective_memory_v1(sys: &SysFs, path: &str) -> Option<i64> {
    let root = bind_v1_controller_root(sys, MEMORY_V1_DIRS, path, "memory.stat")?;
    let stat = read_bound_v1(sys, &root, path, "memory.stat")?;
    let local = read_bound_v1(sys, &root, path, "memory.limit_in_bytes")?;
    let local = parse_v1_capacity_limit(&local)?;
    let hierarchical = parse_exact_stat_value(&stat, "hierarchical_memory_limit")?;
    let hierarchical = normalize_v1_capacity_limit(hierarchical)?;
    match (local, hierarchical) {
        (MemoryLimit::Limited(local), MemoryLimit::Limited(hierarchical))
            if hierarchical <= local =>
        {
            Some(hierarchical)
        }
        (MemoryLimit::Unlimited, MemoryLimit::Limited(hierarchical)) => Some(hierarchical),
        _ => None,
    }
}

fn parse_v1_capacity_limit(content: &str) -> Option<MemoryLimit> {
    let mut fields = content.split_whitespace();
    let limit = fields.next()?.parse::<i64>().ok()?;
    if fields.next().is_some() {
        return None;
    }
    normalize_v1_capacity_limit(limit)
}

fn normalize_v1_capacity_limit(limit: i64) -> Option<MemoryLimit> {
    if limit == -1 || limit >= i64::MAX / 2 {
        Some(MemoryLimit::Unlimited)
    } else {
        (limit >= 0).then_some(MemoryLimit::Limited(limit))
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

fn bind_v1_controller_root(
    sys: &SysFs,
    roots: &[&str],
    path: &str,
    leaf_file: &str,
) -> Option<String> {
    let mut selected = None;
    for root in roots {
        if read_bound_v1(sys, root, path, leaf_file).is_none() {
            continue;
        }
        if selected.is_some() {
            return None;
        }
        selected = Some((*root).to_owned());
    }
    selected
}

fn read_bound_v1(sys: &SysFs, root: &str, path: &str, file: &str) -> Option<String> {
    let relative = path.trim_matches('/');
    let candidate = match (root.is_empty(), relative.is_empty()) {
        (true, true) => format!("{CGROUP_ROOT}/{file}"),
        (true, false) => format!("{CGROUP_ROOT}/{relative}/{file}"),
        (false, true) => format!("{CGROUP_ROOT}/{root}/{file}"),
        (false, false) => format!("{CGROUP_ROOT}/{root}/{relative}/{file}"),
    };
    sys.read(&candidate).ok()
}

/// Collect cgroup v2 or v1 rows from `KRONIKA_SYS_ROOT/fs/cgroup`.
#[must_use]
pub fn collect(sys: &SysFs, ts: i64, clock_ticks_per_sec: i64) -> CgroupCollection {
    if is_v2(sys) {
        collect_v2(sys, ts)
    } else {
        collect_v1(sys, ts, clock_ticks_per_sec)
    }
}

/// Collect bounded metrics for cgroups that contain at least one live process.
///
/// Membership comes from numeric `/proc/<pid>/cgroup` files. It is direct: no
/// cgroup hierarchy traversal or recursive attribution is performed. Candidate
/// overflow rejects the complete workload tick. I/O row overflow rejects only
/// the I/O section so independently complete CPU, memory, and task rows remain.
///
/// # Errors
/// Returns the procfs directory error or a hard candidate/path ceiling error.
pub fn collect_workloads(
    procfs: &ProcFs,
    sys: &SysFs,
    ts: i64,
    clock_ticks_per_sec: i64,
) -> io::Result<CgroupCollection> {
    let mut memberships = WorkloadMemberships::new(sys);
    for pid in procfs.pid_dirs()? {
        let Ok(content) = procfs.read_raw(&format!("{pid}/cgroup")) else {
            // Processes can exit between enumerating /proc and reading their
            // membership. The remaining live snapshot is still coherent.
            continue;
        };
        memberships.observe(&content);
    }
    if let Ok(content) = procfs.read_raw("self/cgroup") {
        memberships.observe(&content);
    }
    memberships.collect(sys, ts, clock_ticks_per_sec)
}

/// Collect bounded metrics from already-read direct process memberships.
///
/// This is the production entry point used to reuse the process collector's
/// `/proc/<pid>/cgroup` reads.
///
/// # Errors
/// Returns a hard candidate/path ceiling error.
pub fn collect_workload_memberships<I, S>(
    memberships: I,
    sys: &SysFs,
    ts: i64,
    clock_ticks_per_sec: i64,
) -> io::Result<CgroupCollection>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut observed = WorkloadMemberships::new(sys);
    for content in memberships {
        observed.observe(content.as_ref());
    }
    observed.collect(sys, ts, clock_ticks_per_sec)
}

fn is_v2(sys: &SysFs) -> bool {
    sys.read(&rel("/", "cgroup.controllers")).is_ok()
        || sys.read(&rel("/", "cpu.stat")).is_ok()
        || sys.read(&rel("/", "memory.current")).is_ok()
        || sys.read(&rel("/", "io.stat")).is_ok()
}

fn collect_v2(sys: &SysFs, ts: i64) -> CgroupCollection {
    collect_v2_paths(sys, ts, discover_v2_paths(sys))
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

fn collect_v1(sys: &SysFs, ts: i64, clock_ticks_per_sec: i64) -> CgroupCollection {
    let paths = discover_v1_paths(sys);
    let workload_paths = WorkloadCgroupPaths {
        cpu: paths.iter().cloned().collect(),
        memory: paths.iter().cloned().collect(),
        io: paths.iter().cloned().collect(),
        pids: paths.into_iter().collect(),
        ..WorkloadCgroupPaths::default()
    };
    collect_v1_paths(sys, ts, clock_ticks_per_sec, workload_paths)
}

fn collect_v1_paths(
    sys: &SysFs,
    ts: i64,
    clock_ticks_per_sec: i64,
    paths: WorkloadCgroupPaths,
) -> CgroupCollection {
    let mut out = CgroupCollection::default();
    for path in paths.cpu {
        if let Some(cpu) = read_cpu_v1(sys, ts, &path, clock_ticks_per_sec) {
            out.cpu.push(cpu);
        }
    }
    for path in paths.memory {
        if let Some(memory) = read_memory_v1(sys, ts, &path) {
            out.memory.push(memory);
        }
    }
    for path in paths.pids {
        if let Some(pids) = read_pids_v1(sys, ts, &path) {
            out.pids.push(pids);
        }
    }
    for path in paths.io {
        if out.io_omitted {
            break;
        }
        let bytes = read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.throttle.io_service_bytes")
            .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.io_service_bytes"));
        let ops = read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.throttle.io_serviced")
            .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.io_serviced"));
        if bytes.is_some() || ops.is_some() {
            let remaining = MAX_CGROUP_IO_ROWS.saturating_sub(out.io.len());
            append_bounded_io(
                &mut out,
                parse_blkio_service_stats_bounded(
                    bytes.as_deref().unwrap_or_default(),
                    ops.as_deref().unwrap_or_default(),
                    ts,
                    &path,
                    remaining,
                ),
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

fn read_cpu_v1(sys: &SysFs, ts: i64, path: &str, clock_ticks_per_sec: i64) -> Option<CgroupCpuRow> {
    let mut row = CgroupCpuRow {
        ts,
        cgroup_path: path.to_owned(),
        usage_usec: 0,
        user_usec: 0,
        system_usec: 0,
        throttled_usec: 0,
        nr_throttled: 0,
        quota_usec: -1,
        period_usec: DEFAULT_CPU_PERIOD_USEC,
    };
    let usage_found =
        read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.usage").is_some_and(|content| {
            row.usage_usec = parse_i64(&content).unwrap_or(0) / 1000;
            true
        });
    let acct_found = read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.stat").is_some_and(|content| {
        parse_cpuacct_stat(&content, clock_ticks_per_sec, &mut row);
        true
    });
    if let Some(content) = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.cfs_quota_us") {
        row.quota_usec = parse_i64(&content).unwrap_or(-1);
    }
    if let Some(content) = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.cfs_period_us") {
        row.period_usec = parse_i64(&content).unwrap_or(DEFAULT_CPU_PERIOD_USEC);
    }
    let throttle_found = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.stat").is_some_and(|content| {
        parse_v1_cpu_stat(&content, &mut row);
        true
    });
    (usage_found || acct_found || throttle_found).then_some(row)
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

fn read_memory_v1(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupMemoryRow> {
    let current = parse_i64(&read_first_v1(
        sys,
        MEMORY_V1_DIRS,
        path,
        "memory.usage_in_bytes",
    )?)?;
    let mut row = CgroupMemoryRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.limit_in_bytes")
            .and_then(|content| parse_v1_memory_limit(&content)),
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
    if let Some(content) = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.stat") {
        parse_memory_stat_v1(&content, &mut row);
    }
    if let Some(content) = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.failcnt") {
        row.max_events = parse_i64(&content).unwrap_or(0);
    }
    Some(row)
}

/// Read one already validated cgroup memory path without scanning cgroupfs.
#[must_use]
pub fn read_memory_path(
    sys: &SysFs,
    ts: i64,
    path: &str,
    unified_v2: bool,
) -> Option<(CgroupMemoryRow, bool)> {
    if unified_v2 {
        let limit = sys.read(&rel(path, "memory.max")).ok()?;
        if limit != "max" && parse_i64(&limit).is_none() {
            return None;
        }
        Some((read_memory_v2(sys, ts, path)?, limit == "max"))
    } else {
        let limit = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.limit_in_bytes")?;
        parse_i64(&limit)?;
        let unlimited = parse_v1_memory_limit(&limit).is_none();
        Some((read_memory_v1(sys, ts, path)?, unlimited))
    }
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

fn read_pids_v1(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupPidsRow> {
    let current = read_first_v1(sys, PIDS_V1_DIRS, path, "pids.current")?;
    let max = read_first_v1(sys, PIDS_V1_DIRS, path, "pids.max")?;
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

fn discover_v2_paths(sys: &SysFs) -> Vec<String> {
    discover_tree(sys, CGROUP_ROOT, "/")
}

fn discover_v1_paths(sys: &SysFs) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for controller in [
        "cpu,cpuacct",
        "cpuacct,cpu",
        "cpu",
        "cpuacct",
        "memory",
        "pids",
        "blkio",
    ] {
        // One cgroup appears under every controller mounted for it, and must
        // still produce one row.
        paths.extend(discover_tree(
            sys,
            &format!("{CGROUP_ROOT}/{controller}"),
            "/",
        ));
    }
    if paths.is_empty()
        && ["cpuacct.usage", "memory.usage_in_bytes", "pids.current"]
            .iter()
            .any(|file| sys.read(&format!("{CGROUP_ROOT}/{file}")).is_ok())
    {
        paths.insert("/".to_owned());
    }
    paths.into_iter().collect()
}

fn discover_tree(sys: &SysFs, base_rel: &str, root_path: &str) -> Vec<String> {
    let Ok(root_children) = sys.read_dir(base_rel) else {
        return Vec::new();
    };

    let mut out = vec![normalize_path(root_path, "")];
    let mut queue: VecDeque<String> = root_children
        .into_iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.name)
        .collect();
    while let Some(relative) = queue.pop_front() {
        out.push(normalize_path(root_path, &relative));
        let rel = format!("{base_rel}/{relative}");
        let Ok(children) = sys.read_dir(&rel) else {
            continue;
        };
        for child in children.into_iter().filter(|entry| entry.is_dir) {
            queue.push_back(format!("{relative}/{}", child.name));
        }
    }
    out
}

fn normalize_path(root_path: &str, relative: &str) -> String {
    if relative.is_empty() {
        root_path.to_owned()
    } else if root_path == "/" {
        format!("/{relative}")
    } else {
        format!("{root_path}/{relative}")
    }
}

fn rel(path: &str, file: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("{CGROUP_ROOT}/{file}")
    } else {
        format!("{CGROUP_ROOT}/{path}/{file}")
    }
}

fn read_first_v1(sys: &SysFs, dirs: &[&str], path: &str, file: &str) -> Option<String> {
    let relative = path.trim_matches('/');
    for dir in dirs {
        let candidate = match (dir.is_empty(), relative.is_empty()) {
            (true, true) => format!("{CGROUP_ROOT}/{file}"),
            (true, false) => format!("{CGROUP_ROOT}/{relative}/{file}"),
            (false, true) => format!("{CGROUP_ROOT}/{dir}/{file}"),
            (false, false) => format!("{CGROUP_ROOT}/{dir}/{relative}/{file}"),
        };
        if let Ok(content) = sys.read(&candidate) {
            return Some(content);
        }
    }
    None
}

#[cfg(test)]
mod tests;
