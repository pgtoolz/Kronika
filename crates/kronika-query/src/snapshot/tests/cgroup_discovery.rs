use super::*;

use kronika_registry::os_cgroup_v2_group::OsCgroupV2Group;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;

const SCAN: i64 = SEGMENT_ID;
const GROUPS_PER_PART: u32 = 75;
const PARTS_PER_SEGMENT: u32 = 2;
const SEGMENTS: u32 = 4;

fn discovery_part(interner: &mut Interner, first_group: u32) -> Vec<u8> {
    let mut buffers = SectionBuffers::new();
    let mount_root = StrId(interner.intern(b"/").expect("root").get());
    for group in first_group..first_group + GROUPS_PER_PART {
        let path = format!("/group-{group}");
        let identity = format!("cgroup-object-{group}");
        let cgroup_path = StrId(interner.intern(path.as_bytes()).expect("path").get());
        let cgroup_identity = StrId(
            interner
                .intern(identity.as_bytes())
                .expect("identity")
                .get(),
        );
        for ts in [SCAN - 1_000_000, SCAN] {
            buffers
                .push(OsCgroupV2Group {
                    ts: Ts(ts),
                    cgroup_path,
                    cgroup_identity,
                    mount_root,
                    parent_identity: None,
                    memory_localevents: false,
                    pids_localevents: false,
                })
                .expect("inventory row");
            for minor in 0..2 {
                buffers
                    .push(OsCgroupV2Io {
                        ts: Ts(ts),
                        cgroup_path,
                        cgroup_identity,
                        major: 8,
                        minor,
                        rbytes: Some(if ts == SCAN {
                            i64::from(group + minor + 1)
                        } else {
                            0
                        }),
                        wbytes: None,
                        rios: None,
                        wios: None,
                    })
                    .expect("I/O row");
            }
        }
    }
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("nonempty part")
}

fn discovery_source() -> (tempfile::TempDir, PosixSource, i64) {
    let root = tempfile::tempdir().expect("discovery storage");
    let owner = DataRoot::open(root.path())
        .expect("data root")
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut latest = SCAN;
    for segment in 0..SEGMENTS {
        latest = SCAN + i64::from(segment + 1);
        let id = SegmentId::new(latest).expect("distinct opening ID");
        let mut interner = Interner::new(DictLimits::default());
        for part in 0..PARTS_PER_SEGMENT {
            let first = (segment * PARTS_PER_SEGMENT + part) * GROUPS_PER_PART;
            let encoded = discovery_part(&mut interner, first);
            interner
                .flush_window(|_| journal.append(id, &encoded).map(|_| ()))
                .expect("append real WAL part");
        }
        assert_eq!(journal.parts().len(), 2);
        write_segment(&journal, &owner, SegmentAddress::new(id).expect("address"))
            .expect("seal real WAL parts");
        journal.reset().expect("next segment");
    }
    drop(journal);
    drop(owner);
    let source = PosixSource::open(root.path()).expect("recorded source");
    assert_eq!(source.resources().expect("resources").resources.len(), 4);
    (root, source, latest)
}

fn read_snapshot(
    context: &QueryContext,
    latest: i64,
    section: &str,
    fields: &[&str],
) -> Vec<Value> {
    let mut request = snapshot_request(section, fields);
    request.segment_id = latest;
    request.at = SCAN;
    let mut records = SnapshotRecords::default();
    execute(context, QueryRequest::Snapshot(request))
        .expect("prepare discovery snapshot")
        .stream(&mut records)
        .expect("stream discovery snapshot");
    records
        .0
        .into_iter()
        .filter(|record| record["record"] == "row")
        .collect()
}

#[test]
fn latest_anchor_unites_discovery_portions_and_rates_across_four_segments() {
    let (_root, source, latest) = discovery_source();
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    let expected_timestamp = SCAN.to_string();
    let groups = read_snapshot(&context, latest, "os_cgroup_v2_group", &["cgroup_path"]);
    let expected_groups = SEGMENTS * PARTS_PER_SEGMENT * GROUPS_PER_PART;
    assert_eq!(groups.len(), expected_groups as usize);
    let paths = groups
        .iter()
        .map(|row| row["values"][0].as_str().expect("path"))
        .collect::<HashSet<_>>();
    assert_eq!(paths.len(), expected_groups as usize);
    for group in 0..expected_groups {
        assert!(paths.contains(format!("/group-{group}").as_str()));
    }
    let contributing = groups
        .iter()
        .map(|row| row["segment_id"].as_str().expect("segment ID"))
        .collect::<HashSet<_>>();
    assert_eq!(contributing.len(), SEGMENTS as usize);
    assert!(
        groups
            .iter()
            .all(|row| row["timestamp"].as_str() == Some(expected_timestamp.as_str()))
    );

    let io = read_snapshot(
        &context,
        latest,
        "os_cgroup_v2_io",
        &["cgroup_path", "minor", "rbytes"],
    );
    assert_eq!(io.len(), expected_groups as usize * 2);
    let mut devices = HashSet::new();
    for row in io {
        let values = row["values"].as_array().expect("values");
        let path = values[0].as_str().expect("path");
        let group = path
            .strip_prefix("/group-")
            .expect("group prefix")
            .parse::<u32>()
            .expect("group ID");
        let minor = u32::try_from(values[1].as_u64().expect("minor")).expect("minor fits u32");
        assert!(devices.insert((group, minor)), "no duplicated portion rows");
        assert_eq!(values[2].as_f64(), Some(f64::from(group + minor + 1)));
        assert_eq!(row["timestamp"].as_str(), Some(expected_timestamp.as_str()));
    }
}
