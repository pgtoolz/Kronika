//! Highest readable ancestor selection, independent of controller files.

use std::io;
use std::os::unix::fs::MetadataExt;

use super::parse::{parse_cpuacct_stat, parse_i64};
use super::{
    CgroupCollection, CgroupContextRow, CpuQuota, MAX_CGROUP_IO_ROWS, MemoryLimit, ProcFs, PsiRow,
    SysFs, hierarchy_paths, normalize_self_cgroup_path, parse_cpu_max_strict, parse_cpu_stat,
    parse_io_stat_bounded, parse_pressure_at, parse_self_cgroup, parse_v1_capacity_limit,
};
use crate::proc::stat::ParseError;

/// A directory selected without requiring any controller metric file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedCgroup {
    /// Selected path relative to the visible hierarchy root.
    pub path: String,
    /// Recorded mount boundary, without a host or pod assertion.
    pub root: String,
    /// Directory and bound hierarchy identity for counter continuity.
    pub identity: String,
    base: String,
    version: u8,
    cpu_bandwidth: bool,
}

impl SelectedCgroup {
    fn relative(&self, path: &str, file: &str) -> String {
        let path = path.trim_matches('/');
        if path.is_empty() {
            format!("{}/{file}", self.base)
        } else {
            format!("{}/{path}/{file}", self.base)
        }
    }

    fn is_current(&self, sys: &SysFs) -> bool {
        let relative = format!("{}/{}", self.base, self.path.trim_start_matches('/'));
        let Ok(path) = sys.canonical_path(&relative) else {
            return false;
        };
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        self.identity == directory_identity(&self.base, &self.root, &metadata)
    }

    fn read(&self, sys: &SysFs, file: &str) -> Option<String> {
        sys.read(&self.relative(&self.path, file)).ok()
    }
}

/// Selected controller identities and nullable recorded capacity.
#[derive(Debug, Clone, Default)]
pub struct AncestorContext {
    /// Capacity belongs to the selected aggregate, not `PostgreSQL`.
    pub context: CgroupContextRow,
    /// Selected CPU accounting scope; bandwidth is used only when coherent.
    pub cpu: Option<SelectedCgroup>,
    /// Selected memory scope.
    pub memory: Option<SelectedCgroup>,
    /// Selected I/O scope.
    pub io: Option<SelectedCgroup>,
    /// Selected PID scope.
    pub pids: Option<SelectedCgroup>,
}

#[derive(Clone)]
struct Mount {
    base: String,
    root: String,
    version: u8,
    controllers: Vec<String>,
}

/// Select the highest readable ancestor of the collector in each hierarchy.
///
/// # Errors
/// Returns unreadable/invalid collector membership; unavailable controllers
/// remain absent without borrowing metrics from lower directories.
pub fn collect_ancestor_context(
    procfs: &ProcFs,
    sys: &SysFs,
    ts: i64,
) -> io::Result<AncestorContext> {
    let content = procfs.read_raw("self/cgroup")?;
    let parsed = parse_self_cgroup(&content);
    let mounts = mounts(procfs, sys)?;
    let mut out = AncestorContext {
        context: CgroupContextRow {
            ts,
            ..CgroupContextRow::default()
        },
        ..AncestorContext::default()
    };
    if let Some(path) = parsed.unified {
        if let Some(selected) = select_mount(sys, &mounts, 2, &[], path) {
            out.context.cgroup_version = 2;
            out.cpu = Some(selected.clone());
            out.memory = Some(selected.clone());
            out.io = Some(selected.clone());
            out.pids = Some(selected);
        }
    } else {
        out.context.cgroup_version = 1;
        // Different v1 trees are separate objects even when path text matches.
        out.cpu = parsed.cpuacct.and_then(|path| {
            select_mount(sys, &mounts, 1, &["cpuacct"], path).map(|mut selected| {
                selected.cpu_bandwidth &= parsed.cpu == parsed.cpuacct;
                selected
            })
        });
        out.memory = parsed
            .memory
            .and_then(|path| select_mount(sys, &mounts, 1, &["memory"], path));
        out.io = parsed
            .io
            .and_then(|path| select_mount(sys, &mounts, 1, &["blkio"], path));
        out.pids = parsed
            .pids
            .and_then(|path| select_mount(sys, &mounts, 1, &["pids"], path));
        // Only a co-mounted cpuset controller can constrain this same object.
        if let (Some(cpu), Some(path)) = (&out.cpu, parsed.cpuset)
            && parsed.cpuacct == Some(path)
            && select_mount(sys, &mounts, 1, &["cpuacct", "cpuset"], path)
                .is_some_and(|selected| selected.identity == cpu.identity)
        {
            out.context.cpuset_cpus = cpu
                .read(sys, "cpuset.effective_cpus")
                .and_then(|value| super::parse_cpuset_count(&value));
        }
    }
    if let Some(cpu) = &out.cpu {
        out.context.cpu_path = Some(cpu.path.clone());
        if cpu.version == 2 {
            out.context.cpuset_cpus = cpu
                .read(sys, "cpuset.cpus.effective")
                .and_then(|value| super::parse_cpuset_count(&value));
        }
        if let Some((quota, period)) = observed_cpu_limit(sys, cpu) {
            out.context.effective_cpu_quota_usec = Some(quota);
            out.context.effective_cpu_period_usec = Some(period);
        }
    }
    if let Some(memory) = &out.memory {
        out.context.memory_path = Some(memory.path.clone());
        out.context.effective_memory_max = effective_memory(sys, memory);
    }
    out.context.io_path = out.io.as_ref().map(|group| group.path.clone());
    if out.cpu.is_none() && out.memory.is_none() && out.io.is_none() && out.pids.is_none() {
        out.context.cgroup_version = 0;
    }
    Ok(out)
}

fn mounts(procfs: &ProcFs, sys: &SysFs) -> io::Result<Vec<Mount>> {
    let boundary = sys.canonical_path("fs/cgroup")?;
    let content = procfs.read_raw("self/mountinfo")?;
    let mut out = Vec::new();
    for line in content.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields = left.split_whitespace().collect::<Vec<_>>();
        let tail = right.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 || tail.len() < 3 {
            continue;
        }
        let version = match tail[0] {
            "cgroup2" => 2,
            "cgroup" => 1,
            _ => continue,
        };
        let Some(point) = unescape_mount(fields[4]) else {
            continue;
        };
        let Some(root) = unescape_mount(fields[3]) else {
            continue;
        };
        let Ok(point) = std::fs::canonicalize(point) else {
            continue;
        };
        let Ok(relative) = point.strip_prefix(&boundary) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        if normalize_self_cgroup_path(&root).is_none() {
            continue;
        }
        let base = if relative.is_empty() {
            "fs/cgroup".to_owned()
        } else {
            format!("fs/cgroup/{relative}")
        };
        out.push(Mount {
            base,
            root,
            version,
            controllers: tail[2].split(',').map(str::to_owned).collect(),
        });
    }
    Ok(out)
}

fn unescape_mount(value: &str) -> Option<String> {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let digits = bytes.get(i + 1..i + 4)?;
            if !digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                return None;
            }
            let value = u16::from(digits[0] - b'0') * 64
                + u16::from(digits[1] - b'0') * 8
                + u16::from(digits[2] - b'0');
            out.push(u8::try_from(value).ok()?);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn membership_relative<'a>(root: &str, own: &'a str) -> Option<&'a str> {
    let own = normalize_self_cgroup_path(own)?;
    if root == "/" {
        Some(own)
    } else {
        own.strip_prefix(root)
            .filter(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

fn select_mount(
    sys: &SysFs,
    mounts: &[Mount],
    version: u8,
    controllers: &[&str],
    own: &str,
) -> Option<SelectedCgroup> {
    let mut matching = mounts
        .iter()
        .filter(|mount| {
            mount.version == version
                && membership_relative(&mount.root, own).is_some()
                && controllers
                    .iter()
                    .all(|controller| mount.controllers.iter().any(|name| name == controller))
        })
        .collect::<Vec<_>>();
    // Compatible roots are ancestors of the same membership. Prefer the highest
    // visible root; aliases are accepted only when they bind the same objects.
    matching.sort_by(|a, b| a.root.len().cmp(&b.root.len()).then(a.base.cmp(&b.base)));
    let highest = *matching.first()?;
    for mount in &matching {
        let relative = membership_relative(&highest.root, &mount.root)?;
        let through_highest = format!("{}/{}", highest.base, relative.trim_start_matches('/'));
        let first = std::fs::metadata(sys.canonical_path(&through_highest).ok()?).ok()?;
        let alias = std::fs::metadata(sys.canonical_path(&mount.base).ok()?).ok()?;
        if first.dev() != alias.dev() || first.ino() != alias.ino() {
            return None;
        }
    }
    matching
        .into_iter()
        .filter_map(|mount| select(sys, mount, own))
        .min_by_key(|group| group.root.trim_end_matches('/').len() + group.path.len())
}

fn directory_identity(base: &str, root: &str, metadata: &std::fs::Metadata) -> String {
    format!("{base}:{root}:{}:{}", metadata.dev(), metadata.ino())
}

fn select(sys: &SysFs, mount: &Mount, own: &str) -> Option<SelectedCgroup> {
    let relative = membership_relative(&mount.root, own)?;
    let own = if relative.is_empty() { "/" } else { relative };
    let boundary = sys.canonical_path(&mount.base).ok()?;
    let mut selected = None;
    for path in hierarchy_paths(own)?.into_iter().rev() {
        let relative = format!("{}/{}", mount.base, path.trim_start_matches('/'));
        let Ok(absolute) = sys.canonical_path(&relative) else {
            continue;
        };
        if !absolute.starts_with(&boundary) {
            continue;
        }
        // Directory access is independent of controller-file availability.
        let Ok(directory) = std::fs::File::open(&absolute) else {
            continue;
        };
        let Ok(metadata) = directory.metadata() else {
            continue;
        };
        if !metadata.is_dir() || std::fs::read_dir(&absolute).is_err() {
            continue;
        }
        let identity = directory_identity(&mount.base, &mount.root, &metadata);
        selected = Some(SelectedCgroup {
            path,
            root: mount.root.clone(),
            identity,
            base: mount.base.clone(),
            version: mount.version,
            cpu_bandwidth: mount.version == 2 || mount.controllers.iter().any(|name| name == "cpu"),
        });
    }
    selected
}

// These are observed limits of the selected object, not assertions about
// unseen constraints or PostgreSQL's resource scope.
fn observed_cpu_limit(sys: &SysFs, group: &SelectedCgroup) -> Option<(i64, i64)> {
    if !group.cpu_bandwidth || !group.is_current(sys) {
        return None;
    }
    let mut finite = None;
    let mut unlimited_period = None;
    for path in hierarchy_paths(&group.path)? {
        let quota = if group.version == 2 {
            sys.read(&group.relative(&path, "cpu.max"))
                .ok()
                .and_then(|value| parse_cpu_max_strict(&value))
        } else {
            sys.read(&group.relative(&path, "cpu.cfs_quota_us"))
                .ok()
                .zip(sys.read(&group.relative(&path, "cpu.cfs_period_us")).ok())
                .and_then(|(quota, period)| super::parse_cpu_v1_quota_strict(&quota, &period))
        };
        if let Some(quota) = quota {
            if let CpuQuota::Unlimited { period_usec } = quota {
                unlimited_period = Some(period_usec);
            }
            super::update_effective_cpu(&mut finite, quota);
        }
    }
    if !group.is_current(sys) {
        return None;
    }
    finite.or_else(|| unlimited_period.map(|period| (-1, period)))
}

fn effective_memory(sys: &SysFs, group: &SelectedCgroup) -> Option<i64> {
    if !group.is_current(sys) {
        return None;
    }
    let finite = |limit| match limit {
        MemoryLimit::Limited(value) => Some(value),
        MemoryLimit::Unlimited => None,
    };
    let limit = if group.version == 1 {
        let local = group
            .read(sys, "memory.limit_in_bytes")
            .and_then(|value| parse_v1_capacity_limit(&value))
            .and_then(finite);
        let hierarchical = group
            .read(sys, "memory.stat")
            .and_then(|stat| super::parse_exact_stat_value(&stat, "hierarchical_memory_limit"))
            .and_then(super::normalize_v1_capacity_limit)
            .and_then(finite);
        local.into_iter().chain(hierarchical).min()
    } else {
        hierarchy_paths(&group.path)?
            .into_iter()
            .filter_map(|path| {
                sys.read(&group.relative(&path, "memory.max"))
                    .ok()
                    .and_then(|value| super::parse_memory_max_strict(&value))
                    .and_then(finite)
            })
            .min()
    };
    if !group.is_current(sys) {
        return None;
    }
    limit
}

/// Read one selected aggregate per controller, omitting incomplete non-null rows.
#[must_use]
pub fn collect_ancestor_rows(
    sys: &SysFs,
    selected: &AncestorContext,
    ts: i64,
    hz: i64,
) -> CgroupCollection {
    let mut out = CgroupCollection::default();
    if let Some(group) = &selected.cpu
        && group.is_current(sys)
        && let Some(row) = read_cpu(sys, group, ts, hz)
        && group.is_current(sys)
    {
        out.ancestor_cpu.push(row);
    }
    if let Some(group) = &selected.memory
        && group.is_current(sys)
        && let Some(row) = read_memory(sys, group, ts)
        && group.is_current(sys)
    {
        out.ancestor_memory.push(row);
    }
    if let Some(group) = &selected.io
        && group.is_current(sys)
    {
        if group.version == 2 {
            if let Some(content) = group.read(sys, "io.stat") {
                match parse_io_stat_bounded(&content, ts, &group.path, MAX_CGROUP_IO_ROWS) {
                    Some(rows) => out.io = rows,
                    None => out.io_omitted = true,
                }
            }
        } else if let Some((bytes, ops)) = read_recursive_io(sys, group) {
            match super::parse_blkio_service_stats_bounded(
                bytes.as_deref().unwrap_or_default(),
                ops.as_deref().unwrap_or_default(),
                ts,
                &group.path,
                MAX_CGROUP_IO_ROWS,
            ) {
                Some(rows) => out.io = rows,
                None => out.io_omitted = true,
            }
        }
    }
    if selected
        .io
        .as_ref()
        .is_some_and(|group| !group.is_current(sys))
    {
        out.io.clear();
    }
    if let Some(group) = &selected.pids
        && group.is_current(sys)
        && let Some((current, max)) = group
            .read(sys, "pids.current")
            .zip(group.read(sys, "pids.max"))
            .and_then(|(current, max)| super::parse_pids_values(&current, &max))
        && group.is_current(sys)
    {
        out.pids.push(super::CgroupPidsRow {
            ts,
            cgroup_path: group.path.clone(),
            current,
            max,
        });
    }

    out
}

fn read_recursive_io(
    sys: &SysFs,
    group: &SelectedCgroup,
) -> Option<(Option<String>, Option<String>)> {
    for family in ["blkio.throttle", "blkio"] {
        let bytes = group.read(sys, &format!("{family}.io_service_bytes_recursive"));
        let ops = group.read(sys, &format!("{family}.io_serviced_recursive"));
        // An available family owns both fields. Missing members stay unknown;
        // local-task counters and a different policy family cannot fill them.
        if bytes.is_some() || ops.is_some() {
            return Some((bytes, ops));
        }
    }
    None
}

fn valid_keys(content: &str, keys: &[&str]) -> bool {
    keys.iter()
        .all(|key| super::parse_exact_stat_value(content, key).is_some_and(|value| value >= 0))
}

fn read_cpu(
    sys: &SysFs,
    group: &SelectedCgroup,
    ts: i64,
    hz: i64,
) -> Option<super::AncestorCpuRow> {
    let stat = group.read(sys, "cpu.stat");
    let value = |key| {
        stat.as_deref()
            .and_then(|text| super::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let (usage, user, system, throttled) = if group.version == 2 {
        (
            value("usage_usec")?,
            value("user_usec")?,
            value("system_usec")?,
            value("throttled_usec"),
        )
    } else {
        if hz <= 0 {
            return None;
        }
        let account = group.read(sys, "cpuacct.stat")?;
        if !valid_keys(&account, &["user", "system"]) {
            return None;
        }
        let mut row = parse_cpu_stat("", ts, &group.path);
        parse_cpuacct_stat(&account, hz, &mut row);
        let usage = parse_i64(&group.read(sys, "cpuacct.usage")?)?;
        if usage < 0 {
            return None;
        }
        (
            usage / 1000,
            row.user_usec,
            row.system_usec,
            value("throttled_time").map(|value| value / 1000),
        )
    };
    let quota = if group.version == 2 {
        group
            .read(sys, "cpu.max")
            .and_then(|value| parse_cpu_max_strict(&value))
    } else if group.cpu_bandwidth {
        group
            .read(sys, "cpu.cfs_quota_us")
            .zip(group.read(sys, "cpu.cfs_period_us"))
            .and_then(|(quota, period)| super::parse_cpu_v1_quota_strict(&quota, &period))
    } else {
        None
    };
    let pair = quota.map(|quota| match quota {
        CpuQuota::Unlimited { period_usec } => (-1, period_usec),
        CpuQuota::Limited {
            quota_usec,
            period_usec,
        } => (quota_usec, period_usec),
    });
    Some(super::AncestorCpuRow {
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

fn read_memory(sys: &SysFs, group: &SelectedCgroup, ts: i64) -> Option<super::AncestorMemoryRow> {
    let (current_file, limit_file) = if group.version == 2 {
        ("memory.current", "memory.max")
    } else {
        ("memory.usage_in_bytes", "memory.limit_in_bytes")
    };
    let current = parse_i64(&group.read(sys, current_file)?)?;
    if current < 0 {
        return None;
    }
    let limit = group.read(sys, limit_file).and_then(|value| {
        if group.version == 2 {
            super::parse_memory_max_strict(&value)
        } else {
            parse_v1_capacity_limit(&value)
        }
    });
    let stat = group.read(sys, "memory.stat");
    let events = group.read(sys, "memory.events");
    let stat_value = |key| {
        stat.as_deref()
            .and_then(|text| super::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let event = |key| {
        events
            .as_deref()
            .and_then(|text| super::parse_exact_stat_value(text, key))
            .filter(|value| *value >= 0)
    };
    let (anon, file, kernel, slab) = if group.version == 2 {
        (
            stat_value("anon"),
            stat_value("file"),
            stat_value("kernel"),
            stat_value("slab"),
        )
    } else {
        // Total keys are hierarchical; never replace them with local rss/cache.
        let slab = stat_value("total_slab");
        let kernel = slab
            .zip(stat_value("total_kernel_stack"))
            .and_then(|(slab, stack)| slab.checked_add(stack));
        (
            stat_value("total_rss"),
            stat_value("total_cache"),
            kernel,
            slab,
        )
    };
    Some(super::AncestorMemoryRow {
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
        max_events: if group.version == 2 {
            event("max")
        } else {
            group
                .read(sys, "memory.failcnt")
                .and_then(|value| parse_i64(&value))
                .filter(|value| *value >= 0)
        },
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
    let Some(group) = selected.cpu.as_ref().filter(|group| group.version == 2) else {
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
    )
}

/// Device IDs charged to the selected v2 ancestor, without a child fallback.
#[must_use]
pub fn charged_ancestor_devices(sys: &SysFs, selected: &AncestorContext) -> Vec<(i32, i32)> {
    let Some(group) = selected.io.as_ref().filter(|group| group.version == 2) else {
        return Vec::new();
    };
    if !group.is_current(sys) {
        return Vec::new();
    }
    let Some(content) = group.read(sys, "io.stat") else {
        return Vec::new();
    };
    if !group.is_current(sys) {
        return Vec::new();
    }
    parse_io_stat_bounded(&content, 0, &group.path, MAX_CGROUP_IO_ROWS)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            Some((
                i32::try_from(row.major).ok()?,
                i32::try_from(row.minor).ok()?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests;
