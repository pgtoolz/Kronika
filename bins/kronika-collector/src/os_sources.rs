//! Coordinate the OS sources due on one tick.
//!
//! Source modules build rows; `rows` holds them until buffering. `io` shares
//! optional-file handling and dictionary-error diagnostics.

mod block_topology;
mod buffering;
mod core;
mod cpufreq;
mod io;
mod kernel;
mod network;
mod process;
mod rows;
mod storage;
mod topology;
mod user_names;

use kronika_source_os::proc::process::ProcessIoCredentials;
use kronika_source_os::{OsScope, ProcFs, SysFs, cgroup, container_device_set, net_scope};
use kronika_writer::Interner;

use crate::cgroup_discovery::CgroupPass;
use crate::scheduler::{DueSet, SourceKind};
use io::log_degraded;

pub(crate) use buffering::push_os_sources;
pub(crate) use rows::OsSources;
pub(crate) use storage::collect_mountinfo;
pub(crate) use user_names::SegmentUserNames;

/// Schedule and attribution shared by all OS sources in one collection pass.
pub(crate) struct OsTick<'a> {
    /// Scope of device-local rows; network and process scopes are selected separately.
    pub(crate) scope: u8,
    pub(crate) ts: i64,
    pub(crate) in_container: bool,
    pub(crate) collect_cgroups: bool,
    pub(crate) collect_psi: bool,
    pub(crate) due: &'a DueSet,
    /// Reuse the cgroup selection and charged devices from this tick's discovery.
    pub(crate) cgroup_pass: Option<&'a CgroupPass>,
}

/// Collect only due sections, interning strings into the current segment.
/// Mounts are read for both core counters and topology, but emitted only when
/// topology is due. Container context also accompanies process-only ticks.
pub(crate) fn collect_os_sources(
    fs: &ProcFs,
    sys: &SysFs,
    process_io: &mut ProcessIoCredentials,
    interner: &mut Interner,
    users: &mut SegmentUserNames,
    tick: &OsTick<'_>,
) -> OsSources {
    let OsTick {
        scope,
        ts,
        in_container,
        due,
        cgroup_pass,
        ..
    } = *tick;
    let mut os = OsSources::default();
    if ![
        SourceKind::OsCore,
        SourceKind::OsMountTopo,
        SourceKind::OsProcesses,
        SourceKind::OsProcessStatus,
        SourceKind::OsCgroup,
        SourceKind::OsCgroupMapping,
    ]
    .into_iter()
    .any(|source| due.has(source))
    {
        return os;
    }

    let selected = selected_cgroup(fs, sys, tick);
    if let Some(selected) = &selected {
        // Record alongside PSI as well as slower cgroup counters, so a selected
        // directory change cannot be hidden between context snapshots.
        match crate::cgroup_discovery::context_section(interner, selected) {
            Ok(row) => os.cgroup_context = Some(row),
            Err(error) => log_degraded(1_205_002, "cgroup/context", &error),
        }
    }
    if due.has(SourceKind::OsCore) {
        core::collect_core_metrics(fs, scope, ts, &mut os);
        if tick.collect_psi && (!in_container || selected.is_some()) {
            core::collect_pressure_rows(
                fs,
                sys,
                scope,
                ts,
                in_container,
                selected.as_ref(),
                &mut os,
            );
        }
    }
    // OsCore needs mountinfo for the container device filter in diskstats;
    // OsMountTopo needs it to build the attribution section rows.
    let device_tick = due.has(SourceKind::OsCore) || due.has(SourceKind::OsMountTopo);
    let mounts = if device_tick {
        storage::mountinfo_entries(fs, sys)
    } else {
        Vec::new()
    };
    // A container's device sections keep the devices its mounts sit on and the
    // layers its selected ancestor charges I/O to; /proc/diskstats and sysfs describe
    // the whole node.
    let kept = (in_container && device_tick).then(|| {
        let mut devices = container_device_set(&mounts);
        if let Some(pass) = cgroup_pass {
            devices.extend(pass.charged_devices.iter().copied());
        } else if let Some(selected) = &selected {
            devices.extend(cgroup::charged_ancestor_devices(sys, selected));
        }
        devices
    });

    if due.has(SourceKind::OsCore) {
        // Counters: disk and network. Network sections carry the pod's
        // network-namespace scope inside a container, not the host scope.
        let net_scope_id = net_scope(in_container).as_u8();
        os.diskstats = storage::collect_diskstats(fs, interner, scope, ts, kept.as_ref());
        os.netdev = network::collect_netdev(fs, sys, interner, net_scope_id, ts);
        network::collect_protocol_counters(fs, net_scope_id, ts, &mut os);
        kernel::collect_kernel_metrics(
            fs,
            interner,
            scope,
            ts,
            os.cpu.len().saturating_sub(1),
            &mut os,
        );
        os.numa = topology::collect_numa(sys, scope, ts);
    }

    if due.has(SourceKind::OsMountTopo) {
        os.mountinfo = collect_mountinfo(interner, scope, ts, &mounts);
        os.topology = topology::collect_topology(fs, sys, interner, scope, ts);
        block_topology::collect_block_topology(sys, scope, ts, kept.as_ref(), &mut os);
    }
    cpufreq::collect_cpufreq(sys, interner, scope, ts, due, &mut os);

    let entity_scope = if in_container {
        OsScope::Container
    } else {
        OsScope::Host
    }
    .as_u8();
    process::collect_process_sections(
        fs,
        process_io,
        interner,
        users,
        &process::ProcessTick {
            scope: entity_scope,
            ts,
            due,
        },
        &mut os,
    );

    os
}

fn selected_cgroup(fs: &ProcFs, sys: &SysFs, tick: &OsTick<'_>) -> Option<cgroup::AncestorContext> {
    if !tick.in_container || !tick.collect_cgroups {
        return None;
    }
    if let Some(pass) = tick.cgroup_pass {
        return Some(pass.selected.clone());
    }
    Some(
        cgroup::collect_ancestor_context(fs, sys, tick.ts).unwrap_or_else(|err| {
            log_degraded(1_205_002, "cgroup/context", &err);
            cgroup::AncestorContext {
                context: cgroup::CgroupContextRow {
                    ts: tick.ts,
                    ..cgroup::CgroupContextRow::default()
                },
                ..cgroup::AncestorContext::default()
            }
        }),
    )
}

#[cfg(test)]
#[path = "tests/os_sources/collection.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/os_sources/fixtures.rs"]
mod fixtures;
