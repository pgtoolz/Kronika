use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, SectionBuffers};

pub(super) fn push(buffers: &mut SectionBuffers, interner: &mut Interner) {
    let earlier = StrId(
        interner
            .intern(b"/legacy-earlier")
            .expect("earlier path")
            .get(),
    );
    let selected = StrId(
        interner
            .intern(b"/legacy-selected")
            .expect("selected path")
            .get(),
    );
    let earlier_identity = StrId(
        interner
            .intern(b"directory:earlier")
            .expect("earlier identity")
            .get(),
    );
    let selected_identity = StrId(
        interner
            .intern(b"directory:selected")
            .expect("selected identity")
            .get(),
    );
    for offset in 0..6 {
        let (path, identity, usage) = if offset < 3 {
            (earlier, earlier_identity, offset * 1_000_000)
        } else {
            (selected, selected_identity, (offset + 7) * 1_000_000)
        };
        let ts = Ts(super::START + offset * 1_000_000);
        buffers
            .push(OsCgroupContextV2 {
                ts,
                cgroup_version: 2,
                cpu_path: Some(path),
                memory_path: None,
                io_path: None,
                cpuset_cpus: Some(8),
                effective_cpu_quota_usec: Some(200_000),
                effective_cpu_period_usec: Some(100_000),
                effective_memory_max: None,
                pids_path: None,
                cpu_identity: Some(identity),
                memory_identity: None,
                io_identity: None,
                pids_identity: None,
                cpu_root: Some(path),
                memory_root: None,
                io_root: None,
                pids_root: None,
                scope: 4,
            })
            .expect("legacy selected context");
        buffers
            .push(OsCgroupCpuV3 {
                ts,
                cgroup_path: path,
                cgroup_identity: identity,
                usage_usec: usage,
                user_usec: usage,
                system_usec: 0,
                throttled_usec: Some(offset * 100),
                nr_throttled: Some(offset),
                quota_usec: Some(200_000),
                period_usec: Some(100_000),
                scope: 4,
            })
            .expect("legacy selected CPU");
    }
}
