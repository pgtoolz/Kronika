//! Conversion to registry section rows.

use kronika_registry::os_cgroup_context::OsCgroupContext;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_cgroup_io::OsCgroupIo;
use kronika_registry::os_cgroup_memory::OsCgroupMemory;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::{StrId, Ts};

use super::model::{CgroupContextRow, CgroupCpuRow, CgroupIoRow, CgroupMemoryRow, CgroupPidsRow};

/// Convert the collector's cgroup context to the registry row.
#[must_use]
pub const fn to_context_section(
    row: &CgroupContextRow,
    scope: u8,
    cpu_path: Option<StrId>,
    memory_path: Option<StrId>,
    io_path: Option<StrId>,
) -> OsCgroupContext {
    OsCgroupContext {
        ts: Ts(row.ts),
        cgroup_version: row.cgroup_version,
        cpu_path,
        memory_path,
        io_path,
        cpuset_cpus: row.cpuset_cpus,
        effective_cpu_quota_usec: row.effective_cpu_quota_usec,
        effective_cpu_period_usec: row.effective_cpu_period_usec,
        effective_memory_max: row.effective_memory_max,
        scope,
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
