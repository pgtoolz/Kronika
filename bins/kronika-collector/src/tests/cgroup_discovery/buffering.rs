use super::*;

use std::collections::BTreeMap;

use kronika_format::{DictLimits, StrId as DictStrId};
use kronika_registry::Section;
use kronika_source_os::cgroup::discovery::{DiscoveredCpu, DiscoveredMemory, DiscoveredPids};
use kronika_source_os::{ProcFs, SysFs};

const SCAN_TS: i64 = 1_788_000_000_000_000;

fn selected_context() -> AncestorContext {
    let dir = tempfile::tempdir().expect("context fixture");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("self")).expect("proc self");
    std::fs::create_dir_all(sys_root.join("fs/cgroup")).expect("visible cgroup root");
    std::fs::write(proc_root.join("self/cgroup"), "0::/pod/collector\n").expect("membership");
    std::fs::write(
        proc_root.join("self/mountinfo"),
        format!(
            "40 1 0:30 /pod {} rw - cgroup2 cgroup rw\n",
            sys_root.join("fs/cgroup").display()
        ),
    )
    .expect("mount binding");
    cgroup::select_ancestor_context(&ProcFs::new(proc_root), &SysFs::new(sys_root), SCAN_TS)
        .expect("selected ancestor")
}

fn section_rows(mut buffers: SectionBuffers) -> BTreeMap<u32, u32> {
    buffers
        .flush_with_summary(&[])
        .expect("encode buffered rows")
        .expect("nonempty buffered prefix")
        .summary
        .sections
        .into_iter()
        .map(|section| (section.type_id, section.rows))
        .collect()
}

#[test]
fn context_section_interns_the_selected_path_identity_and_root_for_every_controller() {
    let selected = selected_context();
    let group = selected.group.as_ref().expect("selected group");
    let mut interner = Interner::new(DictLimits::default());

    let row = context_section(&mut interner, &selected).expect("context section");

    assert_eq!(row.ts.0, SCAN_TS);
    assert_eq!(row.cgroup_version, 2);
    for (fields, expected) in [
        (
            [row.cpu_path, row.memory_path, row.io_path, row.pids_path],
            group.path.as_str(),
        ),
        (
            [
                row.cpu_identity,
                row.memory_identity,
                row.io_identity,
                row.pids_identity,
            ],
            group.identity.as_str(),
        ),
        (
            [row.cpu_root, row.memory_root, row.io_root, row.pids_root],
            group.root.as_str(),
        ),
    ] {
        for field in fields {
            let id = field.expect("interned context field");
            let stored = interner
                .window()
                .strings()
                .find(|(stored_id, _)| stored_id.get() == id.0)
                .map(|(_, bytes)| bytes);
            assert_eq!(stored, Some(expected.as_bytes()));
        }
    }
    assert_eq!(row.cpuset_cpus, None);
    assert_eq!(row.effective_cpu_quota_usec, None);
    assert_eq!(row.effective_cpu_period_usec, None);
    assert_eq!(row.effective_memory_max, None);
    assert_eq!(row.scope, OsScope::Unknown.as_u8());
}

#[test]
fn unavailable_finite_and_unlimited_limits_remain_distinct() {
    assert_eq!(finite_limit(None), (None, None));
    assert_eq!(finite_limit(Some(-2)), (None, None));
    assert_eq!(finite_limit(Some(-1)), (None, Some(true)));
    assert_eq!(finite_limit(Some(0)), (Some(0), Some(false)));
    assert_eq!(finite_limit(Some(42)), (Some(42), Some(false)));
}

#[test]
fn primary_rows_require_their_own_complete_readings() {
    let selected = selected_context();
    let cpu_type = OsCgroupCpuV3::CONTRACT.type_id.get();
    let memory_type = OsCgroupMemoryV3::CONTRACT.type_id.get();
    let pids_type = OsCgroupPids::CONTRACT.type_id.get();
    for (missing, omitted) in [
        ("none", None),
        ("usage_usec", Some(cpu_type)),
        ("user_usec", Some(cpu_type)),
        ("system_usec", Some(cpu_type)),
        ("memory.current", Some(memory_type)),
        ("pids.current", Some(pids_type)),
        ("pids.max", Some(pids_type)),
    ] {
        let mut group = DiscoveredGroup {
            ts: SCAN_TS,
            cgroup_path: "/discovered".to_owned(),
            cgroup_identity: "discovered-identity".to_owned(),
            mount_root: "/".to_owned(),
            parent_identity: None,
            device: 1,
            inode: 2,
            memory_localevents: false,
            pids_localevents: false,
            cpu: DiscoveredCpu::default(),
            memory: DiscoveredMemory::default(),
            pids: DiscoveredPids::default(),
        };
        group.cpu.usage_usec = (missing != "usage_usec").then_some(10);
        group.cpu.user_usec = (missing != "user_usec").then_some(6);
        group.cpu.system_usec = (missing != "system_usec").then_some(4);
        group.memory.current = (missing != "memory.current").then_some(100);
        group.pids.current = (missing != "pids.current").then_some(3);
        group.pids.max = (missing != "pids.max").then_some(-1);
        let mut buffers = SectionBuffers::new();
        let mut interner = Interner::new(DictLimits::default());

        push_group(&mut buffers, &mut interner, &group, Some(&selected))
            .expect("buffer available resource readings");

        let mut expected = BTreeMap::from([
            (OsCgroupV2Group::CONTRACT.type_id.get(), 1),
            (OsCgroupV2Cpu::CONTRACT.type_id.get(), 1),
            (OsCgroupV2Memory::CONTRACT.type_id.get(), 1),
            (OsCgroupV2Pids::CONTRACT.type_id.get(), 1),
            (cpu_type, 1),
            (memory_type, 1),
            (pids_type, 1),
        ]);
        if let Some(omitted) = omitted {
            expected.remove(&omitted);
        }
        assert_eq!(section_rows(buffers), expected, "missing {missing}");
    }
}

#[test]
fn primary_dictionary_failure_preserves_discovered_io_without_interning_later_fields() {
    let selected = selected_context();
    let primary = selected.group.as_ref().expect("selected group");
    let row = DiscoveredIo {
        ts: SCAN_TS,
        cgroup_path: "/discovered".to_owned(),
        cgroup_identity: "discovered-identity".to_owned(),
        major: 8,
        minor: 1,
        rbytes: Some(10),
        wbytes: None,
        rios: Some(1),
        wios: None,
    };
    let limits = DictLimits::new(1, 1)
        .expect("valid string limits")
        .with_max_total_bytes(2)
        .expect("two truncated dictionary values fit");
    let mut interner = Interner::new(limits);
    let mut buffers = SectionBuffers::new();

    let error = push_io(&mut buffers, &mut interner, &row, Some(&selected))
        .expect_err("the selected path exceeds dictionary capacity");

    assert!(error.to_string().contains("intern discovered cgroup"));
    assert_eq!(
        section_rows(buffers),
        BTreeMap::from([(OsCgroupV2Io::CONTRACT.type_id.get(), 1)])
    );
    for value in [&row.cgroup_path, &row.cgroup_identity] {
        assert!(interner.is_interned(DictStrId::of(value.as_bytes()).expect("string ID")));
    }
    for value in [&primary.path, &primary.identity] {
        assert!(!interner.is_interned(DictStrId::of(value.as_bytes()).expect("string ID")));
    }
}
