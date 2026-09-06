use super::{OsSources, Result, SectionBuffers, buffer_row};

/// Buffer every collected OS section into the snapshot window.
///
/// Rows are pre-built with their string ids already interned, so this only
/// moves them into the buffers.
///
/// # Errors
/// Returns an error if a section buffer is full.
pub(crate) fn push_os_sources(buffers: &mut SectionBuffers, os: &OsSources) -> Result<()> {
    for row in &os.cpu {
        buffer_row(buffers, *row)?;
    }
    if let Some(row) = os.stat {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.meminfo {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.loadavg {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.vmstat {
        buffer_row(buffers, row)?;
    }
    for row in &os.psi {
        buffer_row(buffers, *row)?;
    }
    for row in &os.diskstats {
        buffer_row(buffers, *row)?;
    }
    for row in &os.netdev {
        buffer_row(buffers, *row)?;
    }
    if let Some(row) = os.snmp {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.netstat {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.snmp6 {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.kernel_limits {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.nfs_client {
        buffer_row(buffers, row)?;
    }
    if let Some(row) = os.nfs_server {
        buffer_row(buffers, row)?;
    }
    for row in &os.interrupts {
        buffer_row(buffers, *row)?;
    }
    for row in &os.softirq {
        buffer_row(buffers, *row)?;
    }
    for row in &os.numa {
        buffer_row(buffers, *row)?;
    }
    for row in &os.mountinfo {
        buffer_row(buffers, *row)?;
    }
    for row in &os.topology {
        buffer_row(buffers, *row)?;
    }
    for row in &os.block_topology {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cpufreq_policy {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cpufreq {
        buffer_row(buffers, *row)?;
    }
    for row in &os.processes {
        buffer_row(buffers, *row)?;
    }
    for row in &os.users {
        buffer_row(buffers, *row)?;
    }
    for row in &os.process_status {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cgroup_mapping {
        buffer_row(buffers, *row)?;
    }
    if let Some(row) = os.cgroup_context {
        buffer_row(buffers, row)?;
    }
    for row in &os.cgroup_cpu {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cgroup_memory {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cgroup_io {
        buffer_row(buffers, *row)?;
    }
    for row in &os.cgroup_pids {
        buffer_row(buffers, *row)?;
    }
    Ok(())
}
