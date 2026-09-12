//! Conversion to registry section rows.

use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::{OsCgroupCpu, OsCgroupCpuV3};
use kronika_registry::os_cgroup_io::{OsCgroupIo, OsCgroupIoV2};
use kronika_registry::os_cgroup_memory::{OsCgroupMemory, OsCgroupMemoryV3};
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::{StrId, Ts};

use super::model::{CgroupCpuRow, CgroupIoRow, CgroupMemoryRow, CgroupPidsRow};

/// Build the selected-ancestor context; arrays use CPU, memory, I/O, PID order.
#[must_use]
pub const fn to_ancestor_context_section(
    selected: &super::AncestorContext,
    paths: [Option<StrId>; 4],
    identities: [Option<StrId>; 4],
    roots: [Option<StrId>; 4],
) -> OsCgroupContextV2 {
    let row = &selected.context;
    OsCgroupContextV2 {
        ts: Ts(row.ts),
        cgroup_version: row.cgroup_version,
        cpu_path: paths[0],
        memory_path: paths[1],
        io_path: paths[2],
        pids_path: paths[3],
        cpu_identity: identities[0],
        memory_identity: identities[1],
        io_identity: identities[2],
        pids_identity: identities[3],
        cpu_root: roots[0],
        memory_root: roots[1],
        io_root: roots[2],
        pids_root: roots[3],
        cpuset_cpus: row.cpuset_cpus,
        effective_cpu_quota_usec: row.effective_cpu_quota_usec,
        effective_cpu_period_usec: row.effective_cpu_period_usec,
        effective_memory_max: row.effective_memory_max,
        scope: crate::OsScope::Unknown.as_u8(),
    }
}

/// Convert a CPU row to the registry row.
#[must_use]
pub const fn to_cpu_section(row: &CgroupCpuRow, scope: u8, cgroup_path: StrId) -> OsCgroupCpu {
    OsCgroupCpu {
        ts: Ts(row.ts),
        cgroup_path,
        usage_usec: row.usage_usec,
        user_usec: row.user_usec,
        system_usec: row.system_usec,
        throttled_usec: row.throttled_usec,
        nr_throttled: row.nr_throttled,
        quota_usec: row.quota_usec,
        period_usec: row.period_usec,
        scope,
    }
}

/// Convert a memory row to the registry row.
#[must_use]
pub const fn to_memory_section(
    row: &CgroupMemoryRow,
    scope: u8,
    cgroup_path: StrId,
) -> OsCgroupMemory {
    OsCgroupMemory {
        ts: Ts(row.ts),
        cgroup_path,
        current: row.current,
        max: row.max,
        anon: row.anon,
        file: row.file,
        kernel: row.kernel,
        slab: row.slab,
        low_events: row.low_events,
        high_events: row.high_events,
        max_events: row.max_events,
        oom_events: row.oom_events,
        oom_kill: row.oom_kill,
        scope,
    }
}

/// Convert an I/O row to the registry row.
#[must_use]
pub const fn to_io_section(row: &CgroupIoRow, scope: u8, cgroup_path: StrId) -> OsCgroupIo {
    OsCgroupIo {
        ts: Ts(row.ts),
        cgroup_path,
        major: row.major,
        minor: row.minor,
        rbytes: row.rbytes,
        wbytes: row.wbytes,
        rios: row.rios,
        wios: row.wios,
        scope,
    }
}

/// Convert a PIDs row to the registry row.
#[must_use]
pub const fn to_pids_section(row: &CgroupPidsRow, scope: u8, cgroup_path: StrId) -> OsCgroupPids {
    OsCgroupPids {
        ts: Ts(row.ts),
        cgroup_path,
        current: row.current,
        max: row.max,
        scope,
    }
}

/// Preserve unknown ancestor-memory fields through the registered V3 section.
#[must_use]
pub const fn to_ancestor_memory_section(
    row: &super::AncestorMemoryRow,
    cgroup_path: StrId,
    cgroup_identity: StrId,
) -> OsCgroupMemoryV3 {
    OsCgroupMemoryV3 {
        ts: Ts(row.ts),
        cgroup_path,
        cgroup_identity,
        current: row.current,
        max: row.max,
        max_unlimited: row.max_unlimited,
        anon: row.anon,
        file: row.file,
        kernel: row.kernel,
        slab: row.slab,
        low_events: row.low_events,
        high_events: row.high_events,
        max_events: row.max_events,
        oom_events: row.oom_events,
        oom_kill: row.oom_kill,
        scope: crate::OsScope::Unknown.as_u8(),
    }
}

/// Preserve unavailable ancestor CPU fields through the registered V3 section.
#[must_use]
pub const fn to_ancestor_cpu_section(
    row: &super::AncestorCpuRow,
    cgroup_path: StrId,
    cgroup_identity: StrId,
) -> OsCgroupCpuV3 {
    OsCgroupCpuV3 {
        ts: Ts(row.ts),
        cgroup_path,
        cgroup_identity,
        usage_usec: row.usage_usec,
        user_usec: row.user_usec,
        system_usec: row.system_usec,
        throttled_usec: row.throttled_usec,
        nr_throttled: row.nr_throttled,
        quota_usec: row.quota_usec,
        period_usec: row.period_usec,
        scope: crate::OsScope::Unknown.as_u8(),
    }
}

/// Convert selected-ancestor I/O with its recorded directory identity.
#[must_use]
pub const fn to_ancestor_io_section(
    row: &CgroupIoRow,
    cgroup_path: StrId,
    cgroup_identity: StrId,
) -> OsCgroupIoV2 {
    OsCgroupIoV2 {
        ts: Ts(row.ts),
        cgroup_path,
        cgroup_identity,
        major: row.major,
        minor: row.minor,
        rbytes: row.rbytes,
        wbytes: row.wbytes,
        rios: row.rios,
        wios: row.wios,
        scope: crate::OsScope::Unknown.as_u8(),
    }
}
