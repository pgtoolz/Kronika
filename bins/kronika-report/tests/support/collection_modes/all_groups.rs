use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;
use kronika_registry::os_cgroup_v2_group::OsCgroupV2Group;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;
use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, SectionBuffers};

pub(super) const GROUPS: i64 = 264;

fn intern(interner: &mut Interner, text: &str) -> StrId {
    StrId(
        interner
            .intern(text.as_bytes())
            .expect("fixture label")
            .get(),
    )
}

pub(super) fn push(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    root: StrId,
    root_first: StrId,
    root_second: StrId,
) {
    let jobs = intern(interner, "directory:group-3");
    for group in 0..GROUPS {
        let path = match group {
            0 => root,
            1 => intern(interner, "/visible/api"),
            2 => intern(interner, "/visible/database"),
            3 => intern(interner, "/visible/jobs"),
            _ => intern(interner, &format!("/visible/jobs/worker-{:03}", group - 4)),
        };
        let first = if group == 0 {
            root_first
        } else {
            intern(interner, &format!("directory:group-{group}"))
        };
        let second = if group == 0 {
            root_second
        } else if group == 2 {
            intern(interner, "directory:database-replacement")
        } else {
            first
        };
        for offset in 0..=5 {
            if offset == 3 && group >= GROUPS - 2 {
                continue;
            }
            let identity = if offset < 4 { first } else { second };
            let parent_identity = match group {
                0 => None,
                1..=3 => Some(if offset < 4 { root_first } else { root_second }),
                _ => Some(jobs),
            };
            let ts = Ts(super::START + offset * 1_000_000);
            buffers
                .push(OsCgroupV2Group {
                    ts,
                    cgroup_path: path,
                    cgroup_identity: identity,
                    mount_root: root,
                    parent_identity,
                    memory_localevents: false,
                    pids_localevents: group == 3,
                })
                .expect("group facts");
            let step = if group == 2 && offset >= 4 {
                offset - 4
            } else {
                offset
            };
            let unavailable = group == GROUPS - 1 && offset == 5;
            let factor = if group == 4 { 0 } else { group + 1 };
            push_cpu(buffers, ts, path, identity, factor, step, unavailable);
            push_memory(buffers, ts, path, identity, factor, step, unavailable);
            buffers
                .push(OsCgroupV2Pids {
                    ts,
                    cgroup_path: path,
                    cgroup_identity: identity,
                    current: (!unavailable).then_some(factor + step),
                    max: (!unavailable && group != 3).then_some(256),
                    max_unlimited: (!unavailable).then_some(group == 3),
                    failure_max: (!unavailable).then_some(step * (group % 3)),
                    events_source: if unavailable {
                        0
                    } else if group == 3 {
                        2
                    } else {
                        1
                    },
                })
                .expect("group tasks");
            push_io(buffers, ts, path, identity, factor, step, unavailable);
        }
    }
}

fn push_cpu(
    buffers: &mut SectionBuffers,
    ts: Ts,
    path: StrId,
    identity: StrId,
    factor: i64,
    step: i64,
    unavailable: bool,
) {
    let previous_usage = if factor == 0 {
        0
    } else {
        (GROUPS - factor) * 1_000_000_000
    };
    buffers
        .push(OsCgroupV2Cpu {
            ts,
            cgroup_path: path,
            cgroup_identity: identity,
            usage_usec: (!unavailable).then_some(previous_usage + step * factor * 10_000),
            user_usec: (!unavailable).then_some(previous_usage / 10 * 7 + step * factor * 7_000),
            system_usec: (!unavailable).then_some(previous_usage / 10 * 3 + step * factor * 3_000),
            nr_periods: (!unavailable).then_some(step * 10),
            nr_throttled: (!unavailable).then_some(step * 2),
            throttled_usec: (!unavailable).then_some(step * 25_000),
            quota_usec: (!unavailable).then_some(if factor == 4 { -1 } else { 150_000 }),
            period_usec: (!unavailable).then_some(100_000),
            cpuset_cpus: (!unavailable).then_some(8),
        })
        .expect("group CPU");
}

fn push_memory(
    buffers: &mut SectionBuffers,
    ts: Ts,
    path: StrId,
    identity: StrId,
    factor: i64,
    step: i64,
    unavailable: bool,
) {
    let bytes = factor * 1_048_576;
    buffers
        .push(OsCgroupV2Memory {
            ts,
            cgroup_path: path,
            cgroup_identity: identity,
            current: (!unavailable).then_some(bytes),
            max: (!unavailable && factor != 4).then_some(536_870_912),
            max_unlimited: (!unavailable).then_some(factor == 4),
            high: (!unavailable && factor != 4).then_some(402_653_184),
            high_unlimited: (!unavailable).then_some(factor == 4),
            anon: (!unavailable).then_some(bytes / 2),
            file: (!unavailable).then_some(bytes / 4),
            kernel: (!unavailable).then_some(bytes / 8),
            slab: (!unavailable).then_some(bytes / 16),
            low_events: (!unavailable).then_some(step),
            high_events: (!unavailable).then_some(step * 2),
            max_events: (!unavailable).then_some(step * 3),
            oom_events: (!unavailable).then_some(step * 4),
            oom_kill: (!unavailable).then_some(step * 2),
            local_high_events: (!unavailable).then_some(step),
            local_max_events: (!unavailable).then_some(step * 2),
            local_oom_events: (!unavailable).then_some(step * 3),
            local_oom_kill: (!unavailable).then_some(step),
            local_oom_group_kill: (!unavailable).then_some(0),
        })
        .expect("group memory");
}

fn push_io(
    buffers: &mut SectionBuffers,
    ts: Ts,
    path: StrId,
    identity: StrId,
    factor: i64,
    step: i64,
    unavailable: bool,
) {
    for minor in [0, 16] {
        let device_factor = factor * (minor + 1);
        let previous_bytes = if factor == 0 {
            0
        } else {
            (GROUPS - factor) << 40
        };
        buffers
            .push(OsCgroupV2Io {
                ts,
                cgroup_path: path,
                cgroup_identity: identity,
                major: 8,
                minor: u32::try_from(minor).expect("device minor"),
                rbytes: (!unavailable).then_some(previous_bytes + step * device_factor * 1_048_576),
                wbytes: (!unavailable)
                    .then_some(previous_bytes / 2 + step * device_factor * 524_288),
                rios: (!unavailable).then_some(step * device_factor * 100),
                wios: (!unavailable).then_some(step * device_factor * 50),
            })
            .expect("group device I/O");
    }
}
