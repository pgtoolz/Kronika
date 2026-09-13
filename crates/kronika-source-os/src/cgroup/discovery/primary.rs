//! The one primary ancestor chain, reused within a discovery pass.

use std::collections::HashMap;

use super::{
    CpuQuota, DiscoveryStats, File, MetadataExt, Mode, Mount, OFlags, ProcFs, SysFs, io,
    parse_cpu_max_strict, parse_cpuset_count, read,
};
use crate::cgroup::{AncestorContext, hierarchy_paths};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub cpu: Option<CpuQuota>,
    pub memory: Option<i64>,
    pub cpuset: Option<i64>,
    pub cpuset_read: bool,
}

#[derive(Default)]
pub(super) struct Primary {
    pub limits: HashMap<(u64, u64), Limits>,
    pub selected: AncestorContext,
    pub target: Option<(File, String, Mount)>,
    pub stats: DiscoveryStats,
}

impl Primary {
    pub(super) fn read(
        procfs: &ProcFs,
        sys: &SysFs,
        selected: &AncestorContext,
    ) -> io::Result<Self> {
        let mut out = Self {
            selected: selected.clone(),
            ..Self::default()
        };
        let Some(group) = &selected.group else {
            return Ok(out);
        };
        if !group.is_current(sys) {
            out.selected.group = None;
            out.selected.context = crate::cgroup::CgroupContextRow {
                ts: selected.context.ts,
                ..crate::cgroup::CgroupContextRow::default()
            };
            return Ok(out);
        }
        let Some(mut mount) = super::super::selected::mounts(procfs, sys)?
            .into_iter()
            .find(|mount| mount.base == group.base && mount.root == group.root)
        else {
            return Ok(out);
        };
        mount.base = mount.point.to_string_lossy().into_owned();
        let mut finite_cpu = None;
        let mut unlimited_period = None;
        let mut finite_memory = None;
        for path in hierarchy_paths(&group.path).unwrap_or_default() {
            let absolute = mount.point.join(path.trim_start_matches('/'));
            let directory = match rustix::fs::open(
                &absolute,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(directory) => File::from(directory),
                Err(error) => {
                    out.stats.directory_error(&path, &io::Error::from(error));
                    continue;
                }
            };
            let metadata = directory.metadata()?;
            let cpu = read::scalar(
                &directory,
                &path,
                "cpu.max",
                &mut out.stats,
                parse_cpu_max_strict,
            );
            let memory = read::scalar(&directory, &path, "memory.max", &mut out.stats, read::limit);
            let cpuset = (path == group.path).then(|| {
                read::scalar(
                    &directory,
                    &path,
                    "cpuset.cpus.effective",
                    &mut out.stats,
                    parse_cpuset_count,
                )
            });
            if let Some(cpu) = cpu {
                if let CpuQuota::Unlimited { period_usec } = cpu {
                    unlimited_period = Some(period_usec);
                }
                super::super::update_effective_cpu(&mut finite_cpu, cpu);
            }
            if let Some(memory) = memory.filter(|value| *value >= 0) {
                finite_memory =
                    Some(finite_memory.map_or(memory, |current: i64| current.min(memory)));
            }
            let cpuset_read = path == group.path;
            if cpuset_read {
                out.selected.context.cpuset_cpus = cpuset.flatten();
                out.target = Some((directory, path, mount.clone()));
            }
            out.limits.insert(
                (metadata.dev(), metadata.ino()),
                Limits {
                    cpu,
                    memory,
                    cpuset: cpuset.flatten(),
                    cpuset_read,
                },
            );
        }
        let cpu = finite_cpu.or_else(|| unlimited_period.map(|period| (-1, period)));
        out.selected.context.effective_cpu_quota_usec = cpu.map(|pair| pair.0);
        out.selected.context.effective_cpu_period_usec = cpu.map(|pair| pair.1);
        out.selected.context.effective_memory_max = finite_memory;
        if !group.is_current(sys) {
            out.selected.group = None;
            out.selected.context = crate::cgroup::CgroupContextRow {
                ts: selected.context.ts,
                ..crate::cgroup::CgroupContextRow::default()
            };
            out.target = None;
        }
        Ok(out)
    }
}
