use super::*;
use kronika_registry::os_cgroup_io::OsCgroupIo;
use kronika_registry::os_cgroup_memory::OsCgroupMemory;
use kronika_registry::os_cgroup_pids::OsCgroupPids;
use kronika_registry::os_cgroup_v2_io::OsCgroupV2Io;
use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;

const GROUPS: [(&str, i64); 3] = [("/work/first", 3), ("/work/second", 2), ("/work/needle", 1)];

#[test]
fn discovered_resource_search_forms_match_before_paging() {
    let payload = fixture_payload(|interner, buffers| {
        for (path, value) in GROUPS {
            let path = StrId(interner.intern(path.as_bytes()).expect("path").get());
            for ts in [100, 200] {
                push_discovered(buffers, path, ts, value);
            }
        }
    });
    assert_search_forms(&payload, ["1208001", "1209001", "1210001"]);
}

fn push_discovered(buffers: &mut SectionBuffers, path: StrId, ts: i64, value: i64) {
    buffers
        .push(OsCgroupV2Memory {
            ts: Ts(ts),
            cgroup_path: path,
            cgroup_identity: path,
            current: Some(value),
            max: None,
            max_unlimited: None,
            high: None,
            high_unlimited: None,
            anon: None,
            file: None,
            kernel: None,
            slab: None,
            low_events: None,
            high_events: None,
            max_events: None,
            oom_events: None,
            oom_kill: None,
            local_high_events: None,
            local_max_events: None,
            local_oom_events: None,
            local_oom_kill: None,
            local_oom_group_kill: None,
        })
        .expect("new memory");
    buffers
        .push(OsCgroupV2Pids {
            ts: Ts(ts),
            cgroup_path: path,
            cgroup_identity: path,
            current: Some(value),
            max: None,
            max_unlimited: None,
            failure_max: None,
            events_source: 1,
        })
        .expect("new tasks");
    buffers
        .push(OsCgroupV2Io {
            ts: Ts(ts),
            cgroup_path: path,
            cgroup_identity: path,
            major: 8,
            minor: 0,
            rbytes: Some(value * ts),
            wbytes: None,
            rios: None,
            wios: None,
        })
        .expect("new IO");
}

#[test]
fn old_only_resource_search_forms_match_before_paging() {
    let payload = fixture_payload(|interner, buffers| {
        for (path, value) in GROUPS {
            let path = StrId(interner.intern(path.as_bytes()).expect("path").get());
            for ts in [100, 200] {
                buffers
                    .push(OsCgroupMemory {
                        ts: Ts(ts),
                        cgroup_path: path,
                        current: value,
                        max: None,
                        anon: 0,
                        file: 0,
                        kernel: 0,
                        slab: 0,
                        low_events: 0,
                        high_events: 0,
                        max_events: 0,
                        oom_events: 0,
                        oom_kill: 0,
                        scope: 1,
                    })
                    .expect("old memory");
                buffers
                    .push(OsCgroupPids {
                        ts: Ts(ts),
                        cgroup_path: path,
                        current: value,
                        max: None,
                        scope: 1,
                    })
                    .expect("old tasks");
                buffers
                    .push(OsCgroupIo {
                        ts: Ts(ts),
                        cgroup_path: path,
                        major: 8,
                        minor: 0,
                        rbytes: Some(value * ts),
                        wbytes: None,
                        rios: None,
                        wios: None,
                        scope: 1,
                    })
                    .expect("old IO");
            }
        }
    });
    assert_search_forms(&payload, ["1202001", "1204001", "1203002"]);
}

fn assert_search_forms(payload: &Arc<[u8]>, type_ids: [&str; 3]) {
    for ((resource, metric), type_id) in
        [("memory", "current"), ("pids", "current"), ("io", "rbytes")]
            .into_iter()
            .zip(type_ids)
    {
        let mut request = snapshot_request(&format!("os_cgroup_v2_{resource}"), &["cgroup_path"]);
        request.page_size = Some(1);
        request.by = vec![metric.to_owned()];
        let first = snapshot_records(payload, request.clone());
        let row = first
            .iter()
            .find(|row| row["record"] == "row")
            .expect("first row");
        assert_eq!(row["values"][0], "/work/first");
        let page = first
            .iter()
            .find(|row| row["record"] == "snapshot_page")
            .expect("first page");
        assert_eq!(page["eligible"], "3");
        assert_eq!(page["has_more"], true);
        let mut matching_rows = Vec::new();
        for search in ["needle", "text:needle", "q:needle", "path:needle"] {
            request.search = Some(search.to_owned());
            let records = snapshot_records(payload, request.clone());
            let rows = records
                .iter()
                .filter(|row| row["record"] == "row")
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 1, "{resource} {search}");
            assert_eq!(rows[0]["values"][0], "/work/needle");
            assert_eq!(rows[0]["type_id"], type_id);
            matching_rows.push(rows[0].clone());
            let page = records
                .iter()
                .find(|row| row["record"] == "snapshot_page")
                .expect("searched page");
            assert_eq!(page["eligible"], "1");
            assert_eq!(page["has_more"], false);
        }
        assert!(matching_rows.windows(2).all(|rows| rows[0] == rows[1]));
        request.search = Some("unknown_cgroup_field:needle".to_owned());
        let source = EmbeddedSource::from_owned(
            SegmentId::new(SEGMENT_ID).expect("id"),
            payload.as_ref().to_vec(),
            u64::try_from(payload.len()).expect("length"),
        )
        .expect("source");
        let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
        assert!(matches!(
            execute(&context, QueryRequest::Snapshot(request)),
            Err(crate::QueryError::BadFilter(_))
        ));
    }
}
