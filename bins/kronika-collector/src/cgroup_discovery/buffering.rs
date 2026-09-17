//! Convert discovered and selected cgroups into buffered section rows.

use anyhow::Result;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
use kronika_registry::os_cgroup_io::OsCgroupIoV2;
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV3;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;
use kronika_registry::os_cgroup_v2_group::OsCgroupV2Group;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;
use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
use kronika_registry::{StrId, Ts};
use kronika_source_os::OsScope;
use kronika_source_os::cgroup::discovery::{DiscoveredGroup, DiscoveredIo};
use kronika_source_os::cgroup::{self, AncestorContext};
use kronika_writer::{Interner, SectionBuffers};

use crate::buffering::buffer_row;

// Copy named metric fields into a section and buffer it. Identity fields and
// transformed values stay explicit at each call site; errors return to its `?`.
macro_rules! buffer_fields {
    ($buffers:expr, $section:ident { $($fields:tt)* }, $source:ident [$($copy:ident),+ $(,)?]) => {
        buffer_row($buffers, $section {
            $($fields)*
            $($copy: $source.$copy,)+
        })
    };
}

fn intern(interner: &mut Interner, value: &str) -> Result<StrId> {
    interner
        .intern(value.as_bytes())
        .map(|id| StrId(id.get()))
        .map_err(|error| anyhow::anyhow!("intern discovered cgroup: {error}"))
}

/// Intern the selected group once and use it for all four context controllers.
pub(crate) fn context_section(
    interner: &mut Interner,
    selected: &AncestorContext,
) -> Result<OsCgroupContextV2> {
    let (path, identity, root) = match &selected.group {
        Some(group) => (
            Some(intern(interner, &group.path)?),
            Some(intern(interner, &group.identity)?),
            Some(intern(interner, &group.root)?),
        ),
        None => (None, None, None),
    };
    Ok(cgroup::to_ancestor_context_section(
        selected,
        [path; 4],
        [identity; 4],
        [root; 4],
    ))
}

// The parser uses -1 for `max`. Missing or invalid limits leave both fields null.
fn finite_limit(value: Option<i64>) -> (Option<i64>, Option<bool>) {
    let value = value.filter(|value| *value >= -1);
    (
        value.filter(|value| *value >= 0),
        value.map(|value| value == -1),
    )
}

pub(super) fn push_group(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    group: &DiscoveredGroup,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    let ts = Ts(group.ts);
    let cgroup_path = intern(interner, &group.cgroup_path)?;
    let cgroup_identity = intern(interner, &group.cgroup_identity)?;
    buffer_row(
        buffers,
        OsCgroupV2Group {
            ts,
            cgroup_path,
            cgroup_identity,
            mount_root: intern(interner, &group.mount_root)?,
            parent_identity: group
                .parent_identity
                .as_deref()
                .map(|value| intern(interner, value))
                .transpose()?,
            memory_localevents: group.memory_localevents,
            pids_localevents: group.pids_localevents,
        },
    )?;
    let cpu = &group.cpu;
    buffer_fields!(buffers, OsCgroupV2Cpu {
        ts, cgroup_path, cgroup_identity,
    }, cpu [
        usage_usec, user_usec, system_usec, nr_periods, nr_throttled, throttled_usec,
        quota_usec, period_usec, cpuset_cpus,
    ])?;
    let memory = &group.memory;
    let (max, max_unlimited) = finite_limit(memory.max);
    let (high, high_unlimited) = finite_limit(memory.high);
    buffer_fields!(buffers, OsCgroupV2Memory {
        ts, cgroup_path, cgroup_identity, max, max_unlimited, high, high_unlimited,
    }, memory [
        current, anon, file, kernel, slab, low_events, high_events, max_events,
        oom_events, oom_kill, local_high_events, local_max_events, local_oom_events,
        local_oom_kill, local_oom_group_kill,
    ])?;
    let pids = &group.pids;
    let (max, max_unlimited) = finite_limit(pids.max);
    buffer_fields!(buffers, OsCgroupV2Pids {
        ts, cgroup_path, cgroup_identity, max, max_unlimited,
    }, pids [
        current, failure_max, events_source,
    ])?;
    if let Some(primary) = primary {
        push_primary_group(buffers, interner, group, primary)?;
    }
    Ok(())
}

fn push_primary_group(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    group: &DiscoveredGroup,
    selected: &AncestorContext,
) -> Result<()> {
    let Some(primary) = &selected.group else {
        return Ok(());
    };
    let cgroup_path = intern(interner, &primary.path)?;
    let cgroup_identity = intern(interner, &primary.identity)?;
    let ts = Ts(group.ts);
    let scope = OsScope::Unknown.as_u8();
    let cpu = &group.cpu;
    if let (Some(usage_usec), Some(user_usec), Some(system_usec)) =
        (cpu.usage_usec, cpu.user_usec, cpu.system_usec)
    {
        buffer_fields!(buffers, OsCgroupCpuV3 {
            ts, cgroup_path, cgroup_identity, usage_usec, user_usec, system_usec,
            scope,
        }, cpu [
            throttled_usec, nr_throttled, quota_usec, period_usec,
        ])?;
    }
    let memory = &group.memory;
    if let Some(current) = memory.current {
        let (max, max_unlimited) = finite_limit(memory.max);
        buffer_fields!(buffers, OsCgroupMemoryV3 {
            ts, cgroup_path, cgroup_identity, current, max, max_unlimited, scope,
        }, memory [
            anon, file, kernel, slab, low_events, high_events, max_events, oom_events,
            oom_kill,
        ])?;
    }
    if let (Some(current), Some(max)) = (group.pids.current, group.pids.max) {
        buffer_row(
            buffers,
            OsCgroupPids {
                ts,
                cgroup_path,
                current,
                max: finite_limit(Some(max)).0,
                scope,
            },
        )?;
    }
    Ok(())
}

pub(super) fn push_io(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    row: &DiscoveredIo,
    primary: Option<&AncestorContext>,
) -> Result<()> {
    buffer_fields!(buffers, OsCgroupV2Io {
        ts: Ts(row.ts),
        cgroup_path: intern(interner, &row.cgroup_path)?,
        cgroup_identity: intern(interner, &row.cgroup_identity)?,
    }, row [
        major, minor, rbytes, wbytes, rios, wios,
    ])?;
    if let Some(group) = primary.and_then(|selected| selected.group.as_ref()) {
        buffer_fields!(buffers, OsCgroupIoV2 {
            ts: Ts(row.ts),
            cgroup_path: intern(interner, &group.path)?,
            cgroup_identity: intern(interner, &group.identity)?,
            scope: OsScope::Unknown.as_u8(),
        }, row [
            major, minor, rbytes, wbytes, rios, wios,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/cgroup_discovery/buffering.rs"]
mod tests;
