//! Highest readable ancestor selection, independent of controller files.

use std::io;
use std::os::unix::fs::MetadataExt;

use super::parse::parse_i64;
use super::{
    CgroupCollection, CgroupContextRow, CpuQuota, MAX_CGROUP_IO_ROWS, MemoryLimit, ProcFs, PsiRow,
    SysFs, hierarchy_paths, normalize_self_cgroup_path, parse_cpu_max_strict,
    parse_io_stat_bounded, parse_pressure_at, parse_unified_cgroup_path,
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
    pub(super) base: String,
}

impl SelectedCgroup {
    /// Match the opened directory across equivalent mount aliases, without a file read.
    #[must_use]
    pub fn matches_group(&self, group: &super::discovery::DiscoveredGroup) -> bool {
        let mut fields = self.identity.rsplit(':');
        fields.next().and_then(|value| value.parse::<u64>().ok()) == Some(group.inode)
            && fields.next().and_then(|value| value.parse::<u64>().ok()) == Some(group.device)
    }

    fn relative(&self, path: &str, file: &str) -> String {
        let path = path.trim_matches('/');
        if path.is_empty() {
            format!("{}/{file}", self.base)
        } else {
            format!("{}/{path}/{file}", self.base)
        }
    }

    pub(super) fn is_current(&self, sys: &SysFs) -> bool {
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

/// Selected cgroup v2 identity and nullable recorded capacity.
#[derive(Debug, Clone, Default)]
pub struct AncestorContext {
    /// Capacity belongs to the selected aggregate, not `PostgreSQL`.
    pub context: CgroupContextRow,
    /// One selected cgroup v2 directory for all resource controllers.
    pub group: Option<SelectedCgroup>,
}

#[derive(Clone)]
pub(super) struct Mount {
    pub(super) base: String,
    pub(super) point: std::path::PathBuf,
    pub(super) root: String,
    pub(super) memory_localevents: bool,
    pub(super) pids_localevents: bool,
}

/// Select the highest readable cgroup v2 ancestor of the collector.
///
/// # Errors
/// Unreadable membership or mount information returns an error. Invalid or
/// unsupported unified membership leaves the context empty.
pub fn collect_ancestor_context(
    procfs: &ProcFs,
    sys: &SysFs,
    ts: i64,
) -> io::Result<AncestorContext> {
    let mut out = select_ancestor_context(procfs, sys, ts)?;
    if let Some(group) = &out.group {
        out.context.cgroup_version = 2;
        out.context.cpu_path = Some(group.path.clone());
        out.context.memory_path = Some(group.path.clone());
        out.context.io_path = Some(group.path.clone());
        out.context.cpuset_cpus = group
            .read(sys, "cpuset.cpus.effective")
            .and_then(|value| super::parse_cpuset_count(&value));
        if let Some((quota, period)) = observed_cpu_limit(sys, group) {
            out.context.effective_cpu_quota_usec = Some(quota);
            out.context.effective_cpu_period_usec = Some(period);
        }
        out.context.effective_memory_max = effective_memory(sys, group);
    }
    Ok(out)
}

/// Select the primary directory without opening its resource files.
///
/// # Errors
/// Returns unreadable membership or mount information errors.
pub fn select_ancestor_context(
    procfs: &ProcFs,
    sys: &SysFs,
    ts: i64,
) -> io::Result<AncestorContext> {
    let content = procfs.read_raw("self/cgroup")?;
    let mut out = AncestorContext {
        context: CgroupContextRow {
            ts,
            ..CgroupContextRow::default()
        },
        ..AncestorContext::default()
    };
    let Some(path) = parse_unified_cgroup_path(&content) else {
        return Ok(out);
    };
    let mounts = mounts(procfs, sys)?;
    out.group = select_mount(sys, &mounts, path);
    if let Some(group) = &out.group {
        out.context.cgroup_version = 2;
        out.context.cpu_path = Some(group.path.clone());
        out.context.memory_path = Some(group.path.clone());
        out.context.io_path = Some(group.path.clone());
    }
    Ok(out)
}

pub(super) fn mounts(procfs: &ProcFs, sys: &SysFs) -> io::Result<Vec<Mount>> {
    let boundary = sys.canonical_path("fs/cgroup")?;
    let mut out = Vec::new();
    for mut mount in exposed_mounts(procfs)? {
        let Ok(point) = std::fs::canonicalize(&mount.point) else {
            continue;
        };
        let Ok(relative) = point.strip_prefix(&boundary) else {
            continue;
        };
        let Some(relative) = relative.to_str() else {
            continue;
        };
        mount.base = if relative.is_empty() {
            "fs/cgroup".to_owned()
        } else {
            format!("fs/cgroup/{relative}")
        };
        out.push(mount);
    }
    Ok(out)
}

pub(super) fn exposed_mounts(procfs: &ProcFs) -> io::Result<Vec<Mount>> {
    let content = procfs.read_raw("self/mountinfo")?;
    let mut out = Vec::new();
    for line in content.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields = left.split_whitespace().collect::<Vec<_>>();
        let tail = right.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 || tail.len() < 3 || tail[0] != "cgroup2" {
            continue;
        }
        let Some(point) = unescape_mount(fields[4]) else {
            continue;
        };
        let Some(root) = unescape_mount(fields[3]) else {
            continue;
        };
        if !std::path::Path::new(&point).is_absolute()
            || normalize_self_cgroup_path(&root).is_none()
        {
            continue;
        }
        out.push(Mount {
            base: point.clone(),
            point: std::path::PathBuf::from(point),
            root,
            memory_localevents: tail[2]
                .split(',')
                .any(|option| option == "memory_localevents"),
            pids_localevents: tail[2]
                .split(',')
                .any(|option| option == "pids_localevents"),
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

fn select_mount(sys: &SysFs, mounts: &[Mount], own: &str) -> Option<SelectedCgroup> {
    let mut matching = mounts
        .iter()
        .filter_map(|mount| select(sys, mount, own).map(|group| (mount, group)))
        .collect::<Vec<_>>();
    matching.sort_by(|(a, left), (b, right)| {
        (left.root.trim_end_matches('/').len() + left.path.len())
            .cmp(&(right.root.trim_end_matches('/').len() + right.path.len()))
            .then(a.root.len().cmp(&b.root.len()))
            .then(a.base.cmp(&b.base))
    });
    let (highest, _) = matching.first()?;
    for (mount, _) in &matching {
        let (ancestor, descendant) = if highest.root.len() <= mount.root.len() {
            (highest, mount)
        } else {
            (mount, highest)
        };
        let relative = membership_relative(&ancestor.root, &descendant.root)?;
        let through_ancestor = format!("{}/{}", ancestor.base, relative.trim_start_matches('/'));
        let metadata = |path: &str| {
            sys.canonical_path(path)
                .ok()
                .and_then(|path| std::fs::metadata(path).ok())
        };
        // An unreadable alias comparison cannot invalidate an independently
        // readable group. A verified different object remains ambiguous.
        if let (Some(first), Some(alias)) =
            (metadata(&through_ancestor), metadata(&descendant.base))
            && (first.dev() != alias.dev() || first.ino() != alias.ino())
        {
            return None;
        }
    }
    matching.into_iter().next().map(|(_, group)| group)
}

pub(super) fn directory_identity(base: &str, root: &str, metadata: &std::fs::Metadata) -> String {
    format!("{base}:{root}:{}:{}", metadata.dev(), metadata.ino())
}

fn select(sys: &SysFs, mount: &Mount, own: &str) -> Option<SelectedCgroup> {
    let relative = membership_relative(&mount.root, own)?;
    let own = if relative.is_empty() { "/" } else { relative };
    let boundary = sys.canonical_path(&mount.base).ok()?;
    for path in hierarchy_paths(own)? {
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
        return Some(SelectedCgroup {
            path,
            root: mount.root.clone(),
            identity,
            base: mount.base.clone(),
        });
    }
    None
}

// These are observed limits of the selected object, not assertions about
// unseen constraints or PostgreSQL's resource scope.
fn observed_cpu_limit(sys: &SysFs, group: &SelectedCgroup) -> Option<(i64, i64)> {
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
    let limit = hierarchy_paths(&group.path)?
        .into_iter()
        .filter_map(|path| {
            sys.read(&group.relative(&path, "memory.max"))
                .ok()
                .and_then(|value| super::parse_memory_max_strict(&value))
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

fn read_cpu(sys: &SysFs, group: &SelectedCgroup, ts: i64) -> Option<super::AncestorCpuRow> {
    let stat = group.read(sys, "cpu.stat");
    let value = |key| {
        stat.as_deref()
            .and_then(|text| super::parse_exact_stat_value(text, key))
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
    let current = parse_i64(&group.read(sys, "memory.current")?)?;
    if current < 0 {
        return None;
    }
    let limit = group
        .read(sys, "memory.max")
        .and_then(|value| super::parse_memory_max_strict(&value));
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
    let (anon, file, kernel, slab) = (
        stat_value("anon"),
        stat_value("file"),
        stat_value("kernel"),
        stat_value("slab"),
    );
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
    let devices = super::discovery::charged_devices(sys, group).unwrap_or_default();
    if group.is_current(sys) {
        devices
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
