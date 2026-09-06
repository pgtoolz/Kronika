use crate::buffering::buffer_row;
use crate::logging::{
    LogLevel, field, layout_id, log_collection_finish, log_count_degraded, log_event, section_name,
};
use crate::scheduler::{DueSet, SourceKind};
use anyhow::Result;
use kronika_registry::os_block_topology::OsBlockTopology;
use kronika_registry::os_cgroup_context::OsCgroupContext;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_cgroup_io::OsCgroupIo;
use kronika_registry::os_cgroup_mapping::OsCgroupMapping;
use kronika_registry::os_cgroup_memory::OsCgroupMemory;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_cpufreq::{OsCpufreq, OsCpufreqPolicy};
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_interrupts::OsInterrupts;
use kronika_registry::os_kernel_limits::OsKernelLimits;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::os_meminfo::OsMeminfo;
use kronika_registry::os_mountinfo::OsMountinfo;
use kronika_registry::os_netdev::OsNetdev;
use kronika_registry::os_netstat::OsNetstat;
use kronika_registry::os_nfs::{OsNfsClient, OsNfsServer};
use kronika_registry::os_numa::OsNuma;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_process_status::OsProcessStatus;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::os_snmp::OsSnmp;
use kronika_registry::os_snmp6::OsSnmp6;
use kronika_registry::os_softirq::OsSoftirq;
use kronika_registry::os_stat::OsStat;
use kronika_registry::os_topology::OsTopology;
use kronika_registry::os_user::OsUser;
use kronika_registry::os_vmstat::OsVmstat;
use kronika_registry::{StrId, Ts};
use kronika_source_os::proc::cpuinfo;
use kronika_source_os::proc::loadavg::parse_loadavg;
use kronika_source_os::proc::meminfo::parse_meminfo;
use kronika_source_os::proc::pressure::parse_pressure;
use kronika_source_os::proc::process::{
    ProcessError, ProcessIoCredentials, ProcessIoTarget, ProcessReader, process_facts,
};
use kronika_source_os::proc::stat::{parse_cpu, parse_stat_misc};
use kronika_source_os::proc::vmstat::parse_vmstat;
use kronika_source_os::proc::{
    diskstats, interrupts, kernel_limits, net_dev, net_netstat, net_snmp, net_snmp6, nfs,
};
use kronika_source_os::{
    MountEntry, MountStringIds, OsScope, ProcFs, SysFs, cgroup, container_device_set,
    is_kernel_tree_mount, is_pseudo_filesystem, mount_row, net_scope, parse_dev_pair,
    parse_mountinfo,
};
use kronika_source_os::{node_id_from_dir, parse_node_meminfo};
use kronika_writer::{Interner, SectionBuffers};
use std::io::ErrorKind;
use std::time::Instant;

mod block_topology;
mod buffering;
mod cgroups;
mod cpufreq;
mod process;
mod procfs_sections;
mod singletons;
mod user_reference;

pub(crate) use buffering::push_os_sources;
pub(crate) use procfs_sections::collect_mountinfo;
#[cfg(test)]
pub(crate) use procfs_sections::{cpu_max_mhz, cpu_numa_node, net_link_facts, resolve_major_zero};
pub(crate) use user_reference::UserReferences;

/// OS procfs sections collected synchronously in the read phase.
pub(crate) struct OsSources {
    cpu: Vec<OsCpu>,
    stat: Option<OsStat>,
    meminfo: Option<OsMeminfo>,
    loadavg: Option<OsLoadavg>,
    vmstat: Option<OsVmstat>,
    psi: Vec<OsPsi>,
    diskstats: Vec<OsDiskstats>,
    netdev: Vec<OsNetdev>,
    snmp: Option<OsSnmp>,
    netstat: Option<OsNetstat>,
    snmp6: Option<OsSnmp6>,
    kernel_limits: Option<OsKernelLimits>,
    nfs_client: Option<OsNfsClient>,
    nfs_server: Option<OsNfsServer>,
    interrupts: Vec<OsInterrupts>,
    softirq: Vec<OsSoftirq>,
    numa: Vec<OsNuma>,
    mountinfo: Vec<OsMountinfo>,
    topology: Vec<OsTopology>,
    block_topology: Vec<OsBlockTopology>,
    cpufreq_policy: Vec<OsCpufreqPolicy>,
    cpufreq: Vec<OsCpufreq>,
    processes: Vec<OsProcess>,
    users: Vec<OsUser>,
    pending_users: Vec<(u8, u32)>,
    process_status: Vec<OsProcessStatus>,
    cgroup_mapping: Vec<OsCgroupMapping>,
    cgroup_context: Option<OsCgroupContext>,
    cgroup_cpu: Vec<OsCgroupCpu>,
    cgroup_memory: Vec<OsCgroupMemory>,
    cgroup_io: Vec<OsCgroupIo>,
    cgroup_pids: Vec<OsCgroupPids>,
}

impl OsSources {
    const fn empty() -> Self {
        Self {
            cpu: Vec::new(),
            stat: None,
            meminfo: None,
            loadavg: None,
            vmstat: None,
            psi: Vec::new(),
            diskstats: Vec::new(),
            netdev: Vec::new(),
            snmp: None,
            netstat: None,
            snmp6: None,
            kernel_limits: None,
            nfs_client: None,
            nfs_server: None,
            interrupts: Vec::new(),
            softirq: Vec::new(),
            numa: Vec::new(),
            mountinfo: Vec::new(),
            topology: Vec::new(),
            block_topology: Vec::new(),
            cpufreq_policy: Vec::new(),
            cpufreq: Vec::new(),
            processes: Vec::new(),
            users: Vec::new(),
            pending_users: Vec::new(),
            process_status: Vec::new(),
            cgroup_mapping: Vec::new(),
            cgroup_context: None,
            cgroup_cpu: Vec::new(),
            cgroup_memory: Vec::new(),
            cgroup_io: Vec::new(),
            cgroup_pids: Vec::new(),
        }
    }

    pub(crate) fn pending_users(&self) -> &[(u8, u32)] {
        &self.pending_users
    }

    #[cfg(test)]
    pub(crate) const fn diskstats_empty(&self) -> bool {
        self.diskstats.is_empty()
    }

    #[cfg(test)]
    pub(crate) const fn mountinfo_empty(&self) -> bool {
        self.mountinfo.is_empty()
    }

    #[cfg(test)]
    pub(crate) const fn cgroup_context_only(row: OsCgroupContext) -> Self {
        let mut sources = Self::empty();
        sources.cgroup_context = Some(row);
        sources
    }

    #[cfg(test)]
    pub(crate) fn cpufreq_only(policies: Vec<OsCpufreqPolicy>, samples: Vec<OsCpufreq>) -> Self {
        let mut sources = Self::empty();
        sources.cpufreq_policy = policies;
        sources.cpufreq = samples;
        sources
    }

    #[cfg(test)]
    pub(crate) fn storage_only(
        mounts: Vec<OsMountinfo>,
        block_topology: Vec<OsBlockTopology>,
    ) -> Self {
        let mut sources = Self::empty();
        sources.mountinfo = mounts;
        sources.block_topology = block_topology;
        sources
    }

    #[cfg(test)]
    pub(crate) fn users_only(users: Vec<OsUser>) -> Self {
        let mut sources = Self::empty();
        sources.users = users;
        sources
    }

    #[cfg(test)]
    pub(crate) fn cgroups_only(
        cpu: Vec<OsCgroupCpu>,
        memory: Vec<OsCgroupMemory>,
        io: Vec<OsCgroupIo>,
        pids: Vec<OsCgroupPids>,
    ) -> Self {
        let mut sources = Self::empty();
        sources.cgroup_cpu = cpu;
        sources.cgroup_memory = memory;
        sources.cgroup_io = io;
        sources.cgroup_pids = pids;
        sources
    }
}

fn read_optional_os_file(fs: &ProcFs, rel: &'static str, type_id: u32) -> Option<String> {
    match fs.read_raw(rel) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            log_event(
                LogLevel::Warn,
                "collection_degraded",
                &[
                    field("collection", section_name(type_id)),
                    field("type_id", type_id),
                    field("layout_id", layout_id(type_id)),
                    field("source", rel),
                    field("reason", &err),
                ],
            );
            None
        }
    }
}

/// Read every procfs OS section synchronously.
///
/// Counter sections (cpu, stat, meminfo, loadavg, vmstat, pressure, diskstats,
/// netdev, snmp, netstat) are gated on `due.has(SourceKind::OsCore)` and are
/// never emitted on an OsMountTopo-only tick. Pressure comes from procfs on a
/// machine and the collector's exact cgroup v2 path in a container.
/// Mountinfo is parsed on every `OsCore` tick for diskstats attribution and
/// emitted, together with topology, only when
/// `due.has(SourceKind::OsMountTopo)` is true.
/// On file read or parse failure the affected section is skipped and a
/// `collection_degraded` event is logged; zeros are never fabricated. `scope`
/// is the host scope for device-local sections; network sections carry their
/// own `net_scope`.
///
/// The `interner` is the segment's interner: device, interface, and mount
/// strings are interned here so the built rows already hold their `StrId`s.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "independent procfs reads share explicit filesystem, scheduler, dictionary, and segment-reference state"
)]
pub(crate) fn collect_os_sources(
    fs: &ProcFs,
    process_io: &mut ProcessIoCredentials,
    interner: &mut Interner,
    users: &mut UserReferences,
    scope: u8,
    ts: i64,
    in_container: bool,
    due: &DueSet,
) -> OsSources {
    let cgroup_due = in_container && due.has(SourceKind::OsCgroup);
    if !due.has(SourceKind::OsCore)
        && !due.has(SourceKind::OsMountTopo)
        && !due.has(SourceKind::OsProcesses)
        && !due.has(SourceKind::OsProcessStatus)
        && !cgroup_due
        && !due.has(SourceKind::OsCgroupMapping)
    {
        return OsSources::empty();
    }

    let mut os = OsSources::empty();

    let sys = SysFs::from_env();
    if due.has(SourceKind::OsCore) {
        singletons::collect_singletons(fs, &sys, scope, ts, in_container, &mut os);
    }
    // OsCore needs mountinfo for the container device filter in diskstats;
    // OsMountTopo needs it to build the attribution section rows.
    let device_tick = due.has(SourceKind::OsCore) || due.has(SourceKind::OsMountTopo);
    let mounts = if device_tick {
        procfs_sections::mountinfo_entries(fs)
    } else {
        Vec::new()
    };
    // A container's device sections keep the devices its mounts sit on and the
    // layers its own cgroup charges I/O to; /proc/diskstats and sysfs describe
    // the whole node.
    let kept = (in_container && device_tick).then(|| {
        let mut devices = container_device_set(&mounts);
        match cgroup::charged_devices(fs, &sys) {
            Ok(charged) => devices.extend(charged),
            Err(err) => log_degraded(1_108_001, "cgroup/io.stat", &err),
        }
        devices
    });

    if due.has(SourceKind::OsCore) {
        // Counters: disk and network. Network sections carry the pod's
        // network-namespace scope inside a container, not the host scope.
        let net_scope_id = net_scope(in_container).as_u8();
        os.diskstats = procfs_sections::collect_diskstats(fs, interner, scope, ts, kept.as_ref());
        os.netdev = procfs_sections::collect_netdev(fs, &sys, interner, net_scope_id, ts);
        procfs_sections::collect_net_singletons(fs, net_scope_id, ts, &mut os);
        procfs_sections::collect_kernel_singletons(
            fs,
            interner,
            scope,
            net_scope_id,
            ts,
            os.cpu.len().saturating_sub(1),
            &mut os,
        );
        os.numa = procfs_sections::collect_numa(&sys, scope, ts);
    }

    if due.has(SourceKind::OsMountTopo) {
        os.mountinfo = collect_mountinfo(interner, scope, ts, &mounts);
        os.topology = procfs_sections::collect_topology(fs, &sys, interner, scope, ts);
        block_topology::collect_block_topology(&sys, scope, ts, kept.as_ref(), &mut os);
    }
    cpufreq::collect_cpufreq(&sys, interner, scope, ts, due, &mut os);

    let entity_scope = os_entity_scope(in_container);
    let mut process_memberships = cgroup_due.then(|| cgroup::WorkloadMemberships::new(&sys));
    process::collect_process_sections(
        fs,
        process_io,
        interner,
        users,
        entity_scope,
        ts,
        due,
        process_memberships.as_mut(),
        &mut os,
    );
    if let Some(process_memberships) = process_memberships {
        cgroups::collect_cgroup_sections(
            &sys,
            interner,
            entity_scope,
            ts,
            fs,
            due,
            process_memberships,
            &mut os,
        );
    }

    os
}

/// Processes and cgroups describe the container the collector runs in, not
/// the node.
const fn os_entity_scope(in_container: bool) -> u8 {
    if in_container {
        OsScope::Container.as_u8()
    } else {
        OsScope::Host.as_u8()
    }
}

#[cfg(test)]
pub(crate) fn collects_cgroup_metrics(in_container: bool, due: &DueSet) -> bool {
    in_container && due.has(SourceKind::OsCgroup)
}

#[cfg(test)]
pub(crate) fn collect_diskstats_for_test(
    fs: &ProcFs,
    interner: &mut Interner,
    kept: Option<&std::collections::HashSet<(i32, i32)>>,
) -> Vec<OsDiskstats> {
    procfs_sections::collect_diskstats(fs, interner, 0, 0, kept)
}

#[cfg(test)]
pub(crate) fn collect_pressure_for_test(
    fs: &ProcFs,
    sys: &SysFs,
    scope: u8,
    ts: i64,
    in_container: bool,
) -> Vec<OsPsi> {
    let mut os = OsSources::empty();
    singletons::collect_pressure_rows(fs, sys, scope, ts, in_container, &mut os);
    os.psi
}

/// Intern one OS string, logging degradation and returning `None` on failure
/// so the caller skips only the affected row.
fn intern_str(
    interner: &mut Interner,
    type_id: u32,
    origin: &'static str,
    value: &str,
) -> Option<StrId> {
    match interner.intern(value.as_bytes()) {
        Ok(id) => Some(StrId(id.get())),
        Err(err) => {
            log_degraded(type_id, origin, &err);
            None
        }
    }
}

/// Emit a `collection_degraded` event with the section identity and reason.
fn log_degraded(type_id: u32, source: &'static str, reason: &dyn std::fmt::Display) {
    log_event(
        LogLevel::Warn,
        "collection_degraded",
        &[
            field("collection", section_name(type_id)),
            field("type_id", type_id),
            field("layout_id", layout_id(type_id)),
            field("source", source),
            field("reason", reason),
        ],
    );
}
