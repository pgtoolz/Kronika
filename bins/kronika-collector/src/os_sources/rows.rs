//! Rows collected during one OS tick, before their window reaches the WAL.

use kronika_registry::os_block_topology::OsBlockTopology;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_mapping::OsCgroupMapping;
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

/// Collected OS rows with strings already interned in the current segment.
#[derive(Default)]
pub(crate) struct OsSources {
    pub(super) cpu: Vec<OsCpu>,
    pub(super) stat: Option<OsStat>,
    pub(super) meminfo: Option<OsMeminfo>,
    pub(super) loadavg: Option<OsLoadavg>,
    pub(super) vmstat: Option<OsVmstat>,
    pub(super) psi: Vec<OsPsi>,
    pub(super) diskstats: Vec<OsDiskstats>,
    pub(super) netdev: Vec<OsNetdev>,
    pub(super) snmp: Option<OsSnmp>,
    pub(super) netstat: Option<OsNetstat>,
    pub(super) snmp6: Option<OsSnmp6>,
    pub(super) kernel_limits: Option<OsKernelLimits>,
    pub(super) nfs_client: Option<OsNfsClient>,
    pub(super) nfs_server: Option<OsNfsServer>,
    pub(super) interrupts: Vec<OsInterrupts>,
    pub(super) softirq: Vec<OsSoftirq>,
    pub(super) numa: Vec<OsNuma>,
    pub(super) mountinfo: Vec<OsMountinfo>,
    pub(super) topology: Vec<OsTopology>,
    pub(super) block_topology: Vec<OsBlockTopology>,
    pub(super) cpufreq_policy: Vec<OsCpufreqPolicy>,
    pub(super) cpufreq: Vec<OsCpufreq>,
    pub(super) processes: Vec<OsProcess>,
    pub(super) users: Vec<OsUser>,
    pub(super) pending_users: Vec<(u8, u32)>,
    pub(super) process_status: Vec<OsProcessStatus>,
    pub(super) cgroup_mapping: Vec<OsCgroupMapping>,
    pub(super) cgroup_context: Option<OsCgroupContextV2>,
}

impl OsSources {
    /// Omit an unchanged context; return emitted context for confirmation after WAL append.
    pub(crate) fn deduplicate_context(
        &mut self,
        recorded: Option<&OsCgroupContextV2>,
    ) -> Option<OsCgroupContextV2> {
        if self.cgroup_context.as_ref() == recorded {
            self.cgroup_context = None;
        }
        self.cgroup_context
    }

    pub(crate) fn pending_users(&self) -> &[(u8, u32)] {
        &self.pending_users
    }
}
