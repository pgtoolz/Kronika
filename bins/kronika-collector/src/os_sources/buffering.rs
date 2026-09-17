use anyhow::Result;
use kronika_writer::SectionBuffers;

use super::OsSources;
use crate::buffering::buffer_row;

/// Buffer every collected OS section into the snapshot window.
///
/// String IDs are already interned; rows are copied into their section buffers.
///
/// # Errors
/// Returns an error if a section buffer is full.
pub(crate) fn push_os_sources(buffers: &mut SectionBuffers, os: &OsSources) -> Result<()> {
    macro_rules! buffer_sections {
        ($($field:ident),+ $(,)?) => {
            $(
                for row in os.$field.iter().copied() {
                    buffer_row(buffers, row)?;
                }
            )+
        };
    }

    buffer_sections!(
        cpu,
        stat,
        meminfo,
        loadavg,
        vmstat,
        psi,
        diskstats,
        netdev,
        snmp,
        netstat,
        snmp6,
        kernel_limits,
        nfs_client,
        nfs_server,
        interrupts,
        softirq,
        numa,
        mountinfo,
        topology,
        block_topology,
        cpufreq_policy,
        cpufreq,
        processes,
        users,
        process_status,
        cgroup_mapping,
        cgroup_context,
    );
    Ok(())
}
