use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_registry::instance_metadata::InstanceMetadataV3;
use kronika_registry::os_cgroup_context::OsCgroupContextV2;
use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
use kronika_registry::os_cgroup_memory::OsCgroupMemoryV3;
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};

#[path = "collection_modes/all_groups.rs"]
mod all_groups;
#[path = "collection_modes/discovery.rs"]
mod discovery;

pub(super) const START: i64 = 1_709_164_800_000_000;
pub(super) const END: i64 = START + 5_000_001;

#[derive(Clone, Copy)]
pub(super) enum Collection {
    Postgresql(Option<u32>),
    Cgroup,
    SeparatedControllers,
    AllCgroups,
}

#[expect(
    clippy::too_many_lines,
    reason = "one production-written fixture covers collection modes and recorded controller identities"
)]
pub(super) fn encoded(collection: Collection) -> Vec<u8> {
    let directory = tempfile::tempdir().expect("fixture directory");
    let root = DataRoot::open(directory.path()).expect("data root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let label = StrId(interner.intern(b"/visible").expect("path").get());
    let first = StrId(interner.intern(b"directory:first").expect("identity").get());
    let second = StrId(
        interner
            .intern(b"directory:replacement")
            .expect("identity")
            .get(),
    );
    let memory_first = StrId(interner.intern(b"memory:first").expect("identity").get());
    let memory_second = StrId(
        interner
            .intern(b"memory:replacement")
            .expect("identity")
            .get(),
    );
    let separated = matches!(collection, Collection::SeparatedControllers);
    let active = StrId(interner.intern(b"active").expect("state").get());
    let backend = StrId(
        interner
            .intern(b"client backend")
            .expect("backend type")
            .get(),
    );
    let mut buffers = SectionBuffers::new();
    let os = !matches!(collection, Collection::Postgresql(_));
    buffers
        .push(InstanceMetadataV3 {
            ts: Ts(START),
            hostname: os.then_some(label),
            kernel_version: os.then_some(label),
            environment: os.then_some(1),
            clock_ticks_per_sec: os.then_some(100),
            page_size_bytes: os.then_some(4096),
            boot_id: os.then_some(label),
            btime: os.then_some(Ts(1)),
            os_enabled: os,
            postgresql_processes_shared: false,
            postgresql_enabled: !os,
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: match collection {
                Collection::Postgresql(capacity) => capacity,
                Collection::Cgroup | Collection::SeparatedControllers | Collection::AllCgroups => {
                    None
                }
            },
        })
        .expect("metadata");
    match collection {
        Collection::Postgresql(_) => {
            for pid in 1..=5 {
                buffers
                    .push(PgStatActivityV3 {
                        ts: Ts(START + 1_000_000),
                        pid,
                        leader_pid: None,
                        datid: None,
                        datname: None,
                        usename: None,
                        application_name: active,
                        client_addr: active,
                        backend_type: backend,
                        state: Some(active),
                        wait_event_type: None,
                        wait_event: None,
                        query: None,
                        query_id: None,
                        backend_xid_age: None,
                        backend_xmin_age: None,
                        backend_start: Ts(1),
                        xact_start: None,
                        query_start: None,
                        state_change: None,
                    })
                    .expect("PostgreSQL activity");
            }
        }
        Collection::Cgroup | Collection::SeparatedControllers | Collection::AllCgroups => {
            if matches!(collection, Collection::AllCgroups) {
                all_groups::push(&mut buffers, &mut interner, label, first, second);
            } else if !separated {
                discovery::push(&mut buffers, label, first);
            }
            for (offset, usage, quota, identity) in [
                (0, 0, Some(150_000), first),
                (1, 1_500_000, Some(150_000), first),
                (2, 3_000_000, Some(200_000), first),
                (3, 4_500_000, None, first),
                (4, 100_000_000, Some(200_000), second),
                (5, 101_000_000, Some(200_000), second),
            ] {
                let ts = Ts(START + offset * 1_000_000);
                let memory_identity = if offset < 5 {
                    memory_first
                } else {
                    memory_second
                };
                buffers
                    .push(OsCgroupContextV2 {
                        ts,
                        cgroup_version: if separated { 1 } else { 2 },
                        cpu_path: Some(label),
                        memory_path: separated.then_some(label),
                        io_path: None,
                        cpuset_cpus: quota.map(|_| 8),
                        effective_cpu_quota_usec: quota,
                        effective_cpu_period_usec: quota.map(|_| 100_000),
                        effective_memory_max: None,
                        pids_path: None,
                        cpu_identity: Some(identity),
                        memory_identity: separated.then_some(memory_identity),
                        io_identity: None,
                        pids_identity: None,
                        cpu_root: Some(label),
                        memory_root: separated.then_some(label),
                        io_root: None,
                        pids_root: None,
                        scope: 4,
                    })
                    .expect("cgroup context");
                buffers
                    .push(OsCgroupCpuV3 {
                        ts,
                        cgroup_path: label,
                        cgroup_identity: identity,
                        usage_usec: usage,
                        user_usec: usage,
                        system_usec: 0,
                        throttled_usec: None,
                        nr_throttled: None,
                        quota_usec: quota,
                        period_usec: quota.map(|_| 100_000),
                        scope: 4,
                    })
                    .expect("cgroup CPU");
                if separated {
                    buffers
                        .push(OsCgroupMemoryV3 {
                            ts,
                            cgroup_path: label,
                            cgroup_identity: memory_identity,
                            current: 1024,
                            max: Some(2048),
                            anon: None,
                            file: None,
                            kernel: None,
                            slab: None,
                            low_events: None,
                            high_events: None,
                            max_events: None,
                            oom_events: None,
                            oom_kill: Some(if offset < 4 { 1 } else { offset - 2 }),
                            max_unlimited: Some(false),
                            scope: 4,
                        })
                        .expect("recorded controller memory");
                }
            }
        }
    }
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode rows")
        .expect("nonempty rows");
    let address = SegmentAddress::new(SegmentId::new(START).expect("segment id")).expect("address");
    journal.append(address.id, &part).expect("append");
    write_segment(&journal, &owner, address).expect("seal ZMS");
    journal.reset().expect("reset journal");
    let path = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    std::fs::read(path).expect("read sealed ZMS")
}
