//! Convert discovered cgroups into registry rows with synchronous caller admission.

use crate::OsScope;
use crate::cgroup::discovery::{DiscoveredGroup, DiscoveredIo};
use crate::cgroup::{self, AncestorContext};
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

/// One converted row, emitted before any later row is constructed.
#[derive(Debug)]
pub enum DiscoveredSection {
    /// Discovered group identity and controller capabilities.
    Group(OsCgroupV2Group),
    /// Discovered CPU counters.
    Cpu(OsCgroupV2Cpu),
    /// Discovered memory counters and nullable limits.
    Memory(OsCgroupV2Memory),
    /// Discovered process counts and limits.
    Pids(OsCgroupV2Pids),
    /// Discovered per-device I/O.
    Io(OsCgroupV2Io),
    /// Selected-primary CPU compatibility row.
    PrimaryCpu(OsCgroupCpuV3),
    /// Selected-primary memory compatibility row.
    PrimaryMemory(OsCgroupMemoryV3),
    /// Selected-primary process compatibility row.
    PrimaryPids(OsCgroupPids),
    /// Selected-primary I/O compatibility row.
    PrimaryIo(OsCgroupIoV2),
}

macro_rules! emit_fields {
    ($emit:expr, $variant:ident, $section:ident { $($fields:tt)* }, $source:ident [$($copy:ident),+ $(,)?]) => {
        $emit(DiscoveredSection::$variant($section {
            $($fields)*
            $($copy: $source.$copy,)+
        }))
    };
}

/// Intern the selected group once and use it for all four context controllers.
///
/// # Errors
/// Returns the first interning error without attempting later strings.
pub fn context_section<E>(
    selected: &AncestorContext,
    mut intern: impl FnMut(&str) -> Result<StrId, E>,
) -> Result<OsCgroupContextV2, E> {
    let (path, identity, root) = match &selected.group {
        Some(group) => (
            Some(intern(&group.path)?),
            Some(intern(&group.identity)?),
            Some(intern(&group.root)?),
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

/// Emit a group's identity and counters, then its optional primary rows.
///
/// The caller supplies a primary context only after checking group identity.
/// Admission is synchronous: an error prevents all later rows and string reads.
///
/// # Errors
/// Returns the first interning or admission error unchanged.
pub fn emit_group_sections<E>(
    group: &DiscoveredGroup,
    primary: Option<&AncestorContext>,
    mut intern: impl FnMut(&str) -> Result<StrId, E>,
    mut emit: impl FnMut(DiscoveredSection) -> Result<(), E>,
) -> Result<(), E> {
    let ts = Ts(group.ts);
    let cgroup_path = intern(&group.cgroup_path)?;
    let cgroup_identity = intern(&group.cgroup_identity)?;
    emit(DiscoveredSection::Group(OsCgroupV2Group {
        ts,
        cgroup_path,
        cgroup_identity,
        mount_root: intern(&group.mount_root)?,
        parent_identity: group
            .parent_identity
            .as_deref()
            .map(&mut intern)
            .transpose()?,
        memory_localevents: group.memory_localevents,
        pids_localevents: group.pids_localevents,
    }))?;
    let cpu = &group.cpu;
    emit_fields!(emit, Cpu, OsCgroupV2Cpu {
        ts, cgroup_path, cgroup_identity,
    }, cpu [
        usage_usec, user_usec, system_usec, nr_periods, nr_throttled, throttled_usec,
        quota_usec, period_usec, cpuset_cpus,
    ])?;
    let memory = &group.memory;
    let (max, max_unlimited) = finite_limit(memory.max);
    let (high, high_unlimited) = finite_limit(memory.high);
    emit_fields!(emit, Memory, OsCgroupV2Memory {
        ts, cgroup_path, cgroup_identity, max, max_unlimited, high, high_unlimited,
    }, memory [
        current, anon, file, kernel, slab, low_events, high_events, max_events,
        oom_events, oom_kill, local_high_events, local_max_events, local_oom_events,
        local_oom_kill, local_oom_group_kill,
    ])?;
    let pids = &group.pids;
    let (max, max_unlimited) = finite_limit(pids.max);
    emit_fields!(emit, Pids, OsCgroupV2Pids {
        ts, cgroup_path, cgroup_identity, max, max_unlimited,
    }, pids [
        current, failure_max, events_source,
    ])?;
    if let Some(primary) = primary {
        emit_primary_group(group, primary, &mut intern, &mut emit)?;
    }
    Ok(())
}

fn emit_primary_group<E>(
    group: &DiscoveredGroup,
    selected: &AncestorContext,
    intern: &mut impl FnMut(&str) -> Result<StrId, E>,
    emit: &mut impl FnMut(DiscoveredSection) -> Result<(), E>,
) -> Result<(), E> {
    let Some(primary) = &selected.group else {
        return Ok(());
    };
    let cgroup_path = intern(&primary.path)?;
    let cgroup_identity = intern(&primary.identity)?;
    let ts = Ts(group.ts);
    let scope = OsScope::Unknown.as_u8();
    let cpu = &group.cpu;
    if let (Some(usage_usec), Some(user_usec), Some(system_usec)) =
        (cpu.usage_usec, cpu.user_usec, cpu.system_usec)
    {
        emit_fields!(emit, PrimaryCpu, OsCgroupCpuV3 {
            ts, cgroup_path, cgroup_identity, usage_usec, user_usec, system_usec,
            scope,
        }, cpu [
            throttled_usec, nr_throttled, quota_usec, period_usec,
        ])?;
    }
    let memory = &group.memory;
    if let Some(current) = memory.current {
        let (max, max_unlimited) = finite_limit(memory.max);
        emit_fields!(emit, PrimaryMemory, OsCgroupMemoryV3 {
            ts, cgroup_path, cgroup_identity, current, max, max_unlimited, scope,
        }, memory [
            anon, file, kernel, slab, low_events, high_events, max_events, oom_events,
            oom_kill,
        ])?;
    }
    if let (Some(current), Some(max)) = (group.pids.current, group.pids.max) {
        emit(DiscoveredSection::PrimaryPids(OsCgroupPids {
            ts,
            cgroup_path,
            current,
            max: finite_limit(Some(max)).0,
            scope,
        }))?;
    }
    Ok(())
}

/// Emit discovered I/O before its optional selected-primary compatibility row.
///
/// # Errors
/// Returns the first interning or admission error unchanged.
pub fn emit_io_sections<E>(
    row: &DiscoveredIo,
    primary: Option<&AncestorContext>,
    mut intern: impl FnMut(&str) -> Result<StrId, E>,
    mut emit: impl FnMut(DiscoveredSection) -> Result<(), E>,
) -> Result<(), E> {
    emit_fields!(emit, Io, OsCgroupV2Io {
        ts: Ts(row.ts),
        cgroup_path: intern(&row.cgroup_path)?,
        cgroup_identity: intern(&row.cgroup_identity)?,
    }, row [
        major, minor, rbytes, wbytes, rios, wios,
    ])?;
    if let Some(group) = primary.and_then(|selected| selected.group.as_ref()) {
        emit_fields!(emit, PrimaryIo, OsCgroupIoV2 {
            ts: Ts(row.ts),
            cgroup_path: intern(&group.path)?,
            cgroup_identity: intern(&group.identity)?,
            scope: OsScope::Unknown.as_u8(),
        }, row [
            major, minor, rbytes, wbytes, rios, wios,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/cgroup/discovered_sections.rs"]
mod tests;
