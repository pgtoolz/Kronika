use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;
use kronika_registry::os_cgroup_v2_group::OsCgroupV2Group;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;
use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
use kronika_registry::{StrId, Ts};
use kronika_writer::SectionBuffers;

pub(super) fn push(buffers: &mut SectionBuffers, path: StrId, identity: StrId) {
    for (offset, available) in [(0, true), (1_000_000, false)] {
        buffers
            .push(OsCgroupV2Group {
                ts: Ts(super::START + offset),
                cgroup_path: path,
                cgroup_identity: identity,
                mount_root: path,
                parent_identity: None,
                memory_localevents: true,
                pids_localevents: false,
            })
            .expect("discovery fixture row");
        buffers
            .push(OsCgroupV2Cpu {
                ts: Ts(super::START + offset),
                cgroup_path: path,
                cgroup_identity: identity,
                usage_usec: available.then_some(7),
                user_usec: available.then_some(7),
                system_usec: available.then_some(7),
                nr_periods: available.then_some(7),
                nr_throttled: available.then_some(7),
                throttled_usec: available.then_some(7),
                quota_usec: available.then_some(150_000),
                period_usec: available.then_some(100_000),
                cpuset_cpus: available.then_some(8),
            })
            .expect("discovery fixture row");
        buffers
            .push(OsCgroupV2Memory {
                ts: Ts(super::START + offset),
                cgroup_path: path,
                cgroup_identity: identity,
                current: available.then_some(7),
                max: None,
                max_unlimited: available.then_some(true),
                high: available.then_some(7),
                high_unlimited: available.then_some(false),
                anon: available.then_some(7),
                file: available.then_some(7),
                kernel: available.then_some(7),
                slab: available.then_some(7),
                low_events: available.then_some(7),
                high_events: available.then_some(7),
                max_events: available.then_some(7),
                oom_events: available.then_some(7),
                oom_kill: available.then_some(7),
                local_high_events: available.then_some(7),
                local_max_events: available.then_some(7),
                local_oom_events: available.then_some(7),
                local_oom_kill: available.then_some(7),
                local_oom_group_kill: available.then_some(7),
            })
            .expect("discovery fixture row");
        buffers
            .push(OsCgroupV2Pids {
                ts: Ts(super::START + offset),
                cgroup_path: path,
                cgroup_identity: identity,
                current: available.then_some(7),
                max: None,
                max_unlimited: available.then_some(true),
                failure_max: available.then_some(7),
                events_source: if available { 1 } else { 2 },
            })
            .expect("discovery fixture row");
        buffers
            .push(OsCgroupV2Io {
                ts: Ts(super::START + offset),
                cgroup_path: path,
                cgroup_identity: identity,
                major: 8,
                minor: 0,
                rbytes: available.then_some(7),
                wbytes: available.then_some(7),
                rios: available.then_some(7),
                wios: available.then_some(7),
            })
            .expect("discovery fixture row");
    }
}
