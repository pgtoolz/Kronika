use super::finite_limit;

#[test]
fn unavailable_finite_and_unlimited_limits_remain_distinct() {
    assert_eq!(finite_limit(None), (None, None));
    assert_eq!(finite_limit(Some(-2)), (None, None));
    assert_eq!(finite_limit(Some(-1)), (None, Some(true)));
    assert_eq!(finite_limit(Some(0)), (Some(0), Some(false)));
    assert_eq!(finite_limit(Some(42)), (Some(42), Some(false)));
}

use super::{DiscoveredSection, context_section, emit_group_sections, emit_io_sections};
use crate::cgroup::discovery::{
    DiscoveredCpu, DiscoveredGroup, DiscoveredIo, DiscoveredMemory, DiscoveredPids,
};
use crate::cgroup::{AncestorContext, CgroupContextRow, SelectedCgroup};
use kronika_registry::StrId;

fn selected() -> AncestorContext {
    AncestorContext {
        context: CgroupContextRow {
            ts: 41,
            ..CgroupContextRow::default()
        },
        group: Some(SelectedCgroup {
            path: "/primary".to_owned(),
            root: "/".to_owned(),
            identity: "selected:1:2".to_owned(),
            base: "fs/cgroup".to_owned(),
        }),
    }
}

#[test]
fn context_interns_path_identity_root_once_in_order_for_all_controllers() {
    let selected = selected();
    let mut strings = Vec::new();
    let row = context_section(&selected, |value| {
        strings.push(value.to_owned());
        Ok::<_, ()>(StrId(
            u64::try_from(strings.len()).expect("small string list"),
        ))
    })
    .expect("context row");
    assert_eq!(strings, ["/primary", "selected:1:2", "/"]);
    assert_eq!(
        [row.cpu_path, row.memory_path, row.io_path, row.pids_path],
        [Some(StrId(1)); 4]
    );
    assert_eq!(
        [
            row.cpu_identity,
            row.memory_identity,
            row.io_identity,
            row.pids_identity
        ],
        [Some(StrId(2)); 4]
    );
    assert_eq!(
        [row.cpu_root, row.memory_root, row.io_root, row.pids_root],
        [Some(StrId(3)); 4]
    );
}

#[test]
fn io_admission_failure_stops_before_primary_dictionary_values() {
    let row = DiscoveredIo {
        ts: 41,
        cgroup_path: "/discovered".to_owned(),
        cgroup_identity: "discovered:1:2".to_owned(),
        major: 8,
        minor: 1,
        rbytes: Some(9),
        wbytes: None,
        rios: Some(1),
        wios: None,
    };
    let mut strings = Vec::new();
    let mut admitted = 0;
    let result = emit_io_sections(
        &row,
        Some(&selected()),
        |value| {
            strings.push(value.to_owned());
            Ok(StrId(1))
        },
        |section| {
            let DiscoveredSection::Io(row) = section else {
                panic!("discovered I/O must come first")
            };
            assert_eq!((row.rbytes, row.wbytes), (Some(9), None));
            admitted += 1;
            Err("buffer full")
        },
    );
    assert_eq!(result, Err("buffer full"));
    assert_eq!(admitted, 1);
    assert_eq!(strings, ["/discovered", "discovered:1:2"]);
}

#[test]
fn group_sections_preserve_nullable_limits_and_primary_row_completeness() {
    let mut group = DiscoveredGroup {
        ts: 41,
        cgroup_path: "/discovered".to_owned(),
        cgroup_identity: "discovered:1:2".to_owned(),
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
    group.cpu.usage_usec = Some(10);
    group.cpu.user_usec = Some(6);
    group.cpu.system_usec = Some(4);
    group.memory.current = Some(100);
    group.memory.max = Some(-1);
    group.memory.high = Some(-2);
    group.pids.current = Some(2);
    for incomplete in [false, true] {
        group.cpu.system_usec = (!incomplete).then_some(4);
        let mut kinds = Vec::new();
        emit_group_sections(
            &group,
            Some(&selected()),
            |_| Ok::<_, ()>(StrId(7)),
            |section| {
                kinds.push(match section {
                    DiscoveredSection::Group(_) => "group",
                    DiscoveredSection::Cpu(row) => {
                        assert_eq!(row.system_usec, group.cpu.system_usec);
                        "cpu"
                    }
                    DiscoveredSection::Memory(row) => {
                        assert_eq!((row.max, row.max_unlimited), (None, Some(true)));
                        assert_eq!((row.high, row.high_unlimited), (None, None));
                        "memory"
                    }
                    DiscoveredSection::Pids(row) => {
                        assert_eq!((row.max, row.max_unlimited), (None, None));
                        "pids"
                    }
                    DiscoveredSection::PrimaryCpu(row) => {
                        assert_eq!(row.scope, crate::OsScope::Unknown.as_u8());
                        "primary_cpu"
                    }
                    DiscoveredSection::PrimaryMemory(_) => "primary_memory",
                    _ => panic!("incomplete primary pids and unrelated I/O must not be emitted"),
                });
                Ok(())
            },
        )
        .expect("group rows");
        let expected = if incomplete {
            vec!["group", "cpu", "memory", "pids", "primary_memory"]
        } else {
            vec![
                "group",
                "cpu",
                "memory",
                "pids",
                "primary_cpu",
                "primary_memory",
            ]
        };
        assert_eq!(kinds, expected);
    }
}
