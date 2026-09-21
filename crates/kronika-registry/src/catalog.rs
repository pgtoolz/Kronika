//! Registered physical layouts, logical section names, and extension provenance.

use crate::{
    PgBouncerEvents, PgBouncerEventsV2, PgLocksV1, PgLocksV2, PgLogAutovacuum, PgLogCheckpoints,
    PgLogErrors, PgLogLifecycle, PgLogLockWaits, PgLogSlowQueries, PgLogTempFiles, PgPreparedXacts,
    PgSettings, PgStatActivityV1, PgStatActivityV2, PgStatActivityV3, PgStatArchiver,
    PgStatBgwriterV1, PgStatBgwriterV2, PgStatCheckpointerV1, PgStatCheckpointerV2,
    PgStatDatabaseV1, PgStatDatabaseV2, PgStatDatabaseV3, PgStatDatabaseV4, PgStatIoV1, PgStatIoV2,
    PgStatProgressVacuumV1, PgStatProgressVacuumV2, PgStatProgressVacuumV3, PgStatStatementsInfo,
    PgStatStatementsV1, PgStatStatementsV2, PgStatStatementsV3, PgStatStatementsV4,
    PgStatStatementsV5, PgStatStatementsV6, PgStatUserIndexesV1, PgStatUserIndexesV2,
    PgStatUserTablesV1, PgStatUserTablesV2, PgStatUserTablesV3, PgStatUserTablesV4, PgStatWalV1,
    PgStatWalV2, PgStorePlansDatasentinelV1, PgStorePlansInfo, PgStorePlansOsscV1,
    PgStorePlansVadvV1, PgWalStorage, Section, TypeContract, instance_metadata, os_block_topology,
    os_cgroup_context, os_cgroup_cpu, os_cgroup_io, os_cgroup_mapping, os_cgroup_memory,
    os_cgroup_pids, os_cgroup_v2_cpu, os_cgroup_v2_group, os_cgroup_v2_io, os_cgroup_v2_memory,
    os_cgroup_v2_pids, os_cpu, os_cpufreq, os_diskstats, os_interrupts, os_kernel_limits,
    os_loadavg, os_meminfo, os_mountinfo, os_netdev, os_netstat, os_nfs, os_numa, os_process,
    os_process_status, os_psi, os_snmp, os_snmp6, os_softirq, os_stat, os_topology, os_user,
    os_vmstat,
};

/// `type_id` of a `dict.strings` section (decoded directly by readers).
pub const DICT_STRINGS_TYPE_ID: u32 = 3_001_001;
/// `type_id` of a `dict.blobs` section. See [`DICT_STRINGS_TYPE_ID`].
pub const DICT_BLOBS_TYPE_ID: u32 = 3_002_001;

/// Return the registry section name for a raw `type_id`.
#[must_use]
pub fn section_name(type_id: u32) -> Option<&'static str> {
    match type_id {
        DICT_STRINGS_TYPE_ID => Some("dict.strings"),
        DICT_BLOBS_TYPE_ID => Some("dict.blobs"),
        _ => contract(type_id).map(|contract| contract.name),
    }
}

/// Return the stable public section name for a physical `type_id`.
///
/// Most layouts use their registry name directly. The three independently
/// implemented `pg_store_plans` extensions are the only current exception:
/// callers request one logical name and still receive separate physical
/// layouts.
#[must_use]
pub fn logical_section_name(type_id: u32) -> Option<&'static str> {
    match type_id {
        1_003_001 | 1_004_001 | 1_018_001 => Some("pg_store_plans"),
        _ => section_name(type_id),
    }
}

/// Return the implementation provenance of a physical section layout.
///
/// A value is present only where several implementations share one public
/// logical name. This deliberately remains a small compile-time mapping.
#[must_use]
pub const fn section_implementation(type_id: u32) -> Option<&'static str> {
    match type_id {
        1_003_001 => Some("ossc"),
        1_004_001 => Some("vadv"),
        1_018_001 => Some("datasentinel"),
        _ => None,
    }
}

/// Return the exact registry contract for a physical `type_id`.
#[must_use]
pub fn contract(type_id: u32) -> Option<&'static TypeContract> {
    registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
}

/// Every type id known to this build, in registry order.
#[must_use]
pub const fn registry() -> &'static [TypeContract] {
    &[
        instance_metadata::InstanceMetadataV1::CONTRACT,
        instance_metadata::InstanceMetadata::CONTRACT,
        instance_metadata::InstanceMetadataV3::CONTRACT,
        instance_metadata::InstanceMetadataV4::CONTRACT,
        os_process::OsProcess::CONTRACT,
        os_process_status::OsProcessStatus::CONTRACT,
        os_cpu::OsCpu::CONTRACT,
        os_stat::OsStat::CONTRACT,
        os_meminfo::OsMeminfo::CONTRACT,
        os_loadavg::OsLoadavg::CONTRACT,
        os_vmstat::OsVmstat::CONTRACT,
        os_psi::OsPsi::CONTRACT,
        os_diskstats::OsDiskstats::CONTRACT,
        os_netdev::OsNetdev::CONTRACT,
        os_snmp::OsSnmp::CONTRACT,
        os_netstat::OsNetstat::CONTRACT,
        os_mountinfo::OsMountinfo::CONTRACT,
        os_topology::OsTopology::CONTRACT,
        os_cpufreq::OsCpufreqPolicy::CONTRACT,
        os_cpufreq::OsCpufreq::CONTRACT,
        os_block_topology::OsBlockTopology::CONTRACT,
        os_user::OsUser::CONTRACT,
        os_cgroup_mapping::OsCgroupMapping::CONTRACT,
        os_cgroup_cpu::OsCgroupCpu::CONTRACT,
        os_cgroup_cpu::OsCgroupCpuV2::CONTRACT,
        os_cgroup_cpu::OsCgroupCpuV3::CONTRACT,
        os_cgroup_memory::OsCgroupMemory::CONTRACT,
        os_cgroup_memory::OsCgroupMemoryV2::CONTRACT,
        os_cgroup_memory::OsCgroupMemoryV3::CONTRACT,
        os_cgroup_io::OsCgroupIo::CONTRACT,
        os_cgroup_io::OsCgroupIoV2::CONTRACT,
        os_cgroup_pids::OsCgroupPids::CONTRACT,
        os_cgroup_context::OsCgroupContext::CONTRACT,
        os_cgroup_context::OsCgroupContextV2::CONTRACT,
        os_cgroup_v2_group::OsCgroupV2Group::CONTRACT,
        os_cgroup_v2_cpu::OsCgroupV2Cpu::CONTRACT,
        os_cgroup_v2_memory::OsCgroupV2Memory::CONTRACT,
        os_cgroup_v2_pids::OsCgroupV2Pids::CONTRACT,
        os_cgroup_v2_io::OsCgroupV2Io::CONTRACT,
        os_interrupts::OsInterrupts::CONTRACT,
        os_softirq::OsSoftirq::CONTRACT,
        os_kernel_limits::OsKernelLimits::CONTRACT,
        os_numa::OsNuma::CONTRACT,
        os_snmp6::OsSnmp6::CONTRACT,
        os_nfs::OsNfsClient::CONTRACT,
        os_nfs::OsNfsServer::CONTRACT,
        PgSettings::CONTRACT,
        PgStatArchiver::CONTRACT,
        PgStatBgwriterV1::CONTRACT,
        PgStatBgwriterV2::CONTRACT,
        PgPreparedXacts::CONTRACT,
        PgStatDatabaseV4::CONTRACT,
        PgStatDatabaseV3::CONTRACT,
        PgStatDatabaseV2::CONTRACT,
        PgStatDatabaseV1::CONTRACT,
        PgStatIoV1::CONTRACT,
        PgStatIoV2::CONTRACT,
        PgStatActivityV3::CONTRACT,
        PgStatActivityV2::CONTRACT,
        PgStatActivityV1::CONTRACT,
        PgStatProgressVacuumV1::CONTRACT,
        PgStatProgressVacuumV2::CONTRACT,
        PgStatProgressVacuumV3::CONTRACT,
        PgStatUserIndexesV2::CONTRACT,
        PgStatUserIndexesV1::CONTRACT,
        PgStatUserTablesV4::CONTRACT,
        PgStatUserTablesV3::CONTRACT,
        PgStatUserTablesV2::CONTRACT,
        PgStatUserTablesV1::CONTRACT,
        PgStorePlansVadvV1::CONTRACT,
        PgStorePlansOsscV1::CONTRACT,
        PgStorePlansDatasentinelV1::CONTRACT,
        PgStatStatementsInfo::CONTRACT,
        PgStorePlansInfo::CONTRACT,
        PgStatCheckpointerV1::CONTRACT,
        PgStatCheckpointerV2::CONTRACT,
        PgStatStatementsV6::CONTRACT,
        PgStatStatementsV5::CONTRACT,
        PgStatStatementsV4::CONTRACT,
        PgStatStatementsV3::CONTRACT,
        PgStatStatementsV2::CONTRACT,
        PgStatStatementsV1::CONTRACT,
        PgStatWalV1::CONTRACT,
        PgStatWalV2::CONTRACT,
        PgWalStorage::CONTRACT,
        PgLocksV1::CONTRACT,
        PgLocksV2::CONTRACT,
        PgLogErrors::CONTRACT,
        PgLogCheckpoints::CONTRACT,
        PgLogAutovacuum::CONTRACT,
        PgLogSlowQueries::CONTRACT,
        PgLogLockWaits::CONTRACT,
        PgLogLifecycle::CONTRACT,
        PgLogTempFiles::CONTRACT,
        PgBouncerEvents::CONTRACT,
        PgBouncerEventsV2::CONTRACT,
    ]
}
