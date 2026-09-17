use super::*;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_cgroup_v2_cpu::OsCgroupV2Cpu;

fn legacy_cpu(ts: i64, path: StrId, usage: i64) -> OsCgroupCpu {
    OsCgroupCpu {
        ts: Ts(ts),
        cgroup_path: path,
        usage_usec: usage,
        user_usec: usage,
        system_usec: 0,
        throttled_usec: 0,
        nr_throttled: 0,
        quota_usec: 200_000,
        period_usec: 100_000,
        scope: 1,
    }
}

fn discovered_cpu(ts: i64, path: StrId, identity: StrId, usage: Option<i64>) -> OsCgroupV2Cpu {
    OsCgroupV2Cpu {
        ts: Ts(ts),
        cgroup_path: path,
        cgroup_identity: identity,
        usage_usec: usage,
        user_usec: usage,
        system_usec: Some(0),
        nr_periods: Some(ts),
        nr_throttled: Some(ts / 2),
        throttled_usec: Some(ts),
        quota_usec: Some(150_000),
        period_usec: Some(100_000),
        cpuset_cpus: Some(8),
    }
}

fn rows(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect()
}

#[test]
fn cgroup_alias_prefers_latest_family_without_filling_missing_new_fields() {
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        let identity = StrId(interner.intern(b"directory-one").expect("identity").get());
        for ts in [100, 200] {
            buffers.push(legacy_cpu(ts, path, ts)).expect("legacy");
        }
        buffers
            .push(discovered_cpu(200, path, identity, None))
            .expect("new");
    });
    let mut request = snapshot_request(
        "os_cgroup_v2_cpu",
        &["cgroup_path", "usage_usec", "nr_periods", "quota_cores"],
    );
    let latest = snapshot_records(&payload, request.clone());
    let latest = rows(&latest);
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0]["type_id"], "1207001");
    assert!(latest[0]["values"][1].is_null());
    assert_eq!(latest[0]["values"][3], 1.5);
    request.at = 100;
    let old = snapshot_records(&payload, request);
    let old = rows(&old);
    assert_eq!(old.len(), 1);
    assert_eq!(old[0]["type_id"], "1201001");
    assert!(old[0]["values"][2].is_null());
    assert_eq!(old[0]["values"][3], 2.0);
    let legacy = snapshot_records(
        &payload,
        snapshot_request("os_cgroup_cpu", &["cgroup_path"]),
    );
    assert_eq!(
        rows(&legacy)[0]["type_id"],
        "1201001",
        "selected overview request remains physical"
    );
}

#[test]
fn cgroup_page_sorts_rates_before_paging_and_searches_all_paths() {
    let payload = fixture_payload(|interner, buffers| {
        for (path, before, after) in [
            ("/work/a", 1_000, 1_001),
            ("/work/b", 1, 4),
            ("/work/c", 0, 2),
        ] {
            let path = StrId(interner.intern(path.as_bytes()).expect("path").get());
            let identity = path;
            buffers
                .push(discovered_cpu(100, path, identity, Some(before)))
                .expect("before");
            buffers
                .push(discovered_cpu(200, path, identity, Some(after)))
                .expect("after");
        }
    });
    let mut request = snapshot_request(
        "os_cgroup_v2_cpu",
        &["cgroup_path", "usage_usec", "throttled_period_ratio"],
    );
    request.page_size = Some(1);
    request.by = vec!["usage_usec".to_owned()];
    let first = snapshot_records(&payload, request.clone());
    assert_eq!(rows(&first)[0]["values"][0], "/work/b");
    assert_eq!(rows(&first)[0]["values"][1], 30_000.0);
    assert_eq!(rows(&first)[0]["values"][2], 0.5);
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("page");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["has_more"], true);
    request.cursor = page["next_cursor"].as_str().map(str::to_owned);
    let second = snapshot_records(&payload, request.clone());
    assert_eq!(rows(&second)[0]["values"][0], "/work/c");
    request.cursor = None;
    for search in ["/work/a", "text:/work/a", "q:/work/a", "path:/work/a"] {
        request.search = Some(search.to_owned());
        let searched = snapshot_records(&payload, request.clone());
        assert_eq!(rows(&searched).len(), 1);
        assert_eq!(rows(&searched)[0]["values"][0], "/work/a");
    }
}

#[test]
fn cgroup_family_transition_and_recreated_directory_break_rates() {
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        let first = StrId(interner.intern(b"directory-one").expect("identity").get());
        let second = StrId(interner.intern(b"directory-two").expect("identity").get());
        buffers
            .push(discovered_cpu(50, path, first, Some(1)))
            .expect("first");
        buffers
            .push(legacy_cpu(100, path, 10))
            .expect("legacy between");
        buffers
            .push(discovered_cpu(150, path, first, Some(20)))
            .expect("return");
        buffers
            .push(discovered_cpu(200, path, second, Some(30)))
            .expect("recreated");
    });
    let mut request = snapshot_request(
        "os_cgroup_v2_cpu",
        &["usage_usec", "throttled_period_ratio"],
    );
    request.at = 150;
    let transition = snapshot_records(&payload, request.clone());
    assert!(
        rows(&transition)[0]["values"]
            .as_array()
            .expect("values")
            .iter()
            .all(Value::is_null)
    );
    request.at = 200;
    let recreated = snapshot_records(&payload, request);
    assert!(
        rows(&recreated)[0]["values"]
            .as_array()
            .expect("values")
            .iter()
            .all(Value::is_null)
    );
}

#[test]
fn legacy_only_alias_and_return_from_new_family_are_not_joined() {
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        buffers.push(legacy_cpu(50, path, 1)).expect("old");
        buffers
            .push(discovered_cpu(100, path, path, Some(10)))
            .expect("new between");
        buffers.push(legacy_cpu(200, path, 30)).expect("old return");
    });
    let records = snapshot_records(
        &payload,
        snapshot_request("os_cgroup_v2_cpu", &["usage_usec"]),
    );
    assert_eq!(rows(&records)[0]["type_id"], "1201001");
    assert!(rows(&records)[0]["values"][0].is_null());
    let old_only = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/old").expect("path").get());
        buffers.push(legacy_cpu(100, path, 1)).expect("old before");
        buffers.push(legacy_cpu(200, path, 2)).expect("old after");
    });
    let records = snapshot_records(
        &old_only,
        snapshot_request("os_cgroup_v2_cpu", &["usage_usec", "nr_periods"]),
    );
    assert_eq!(rows(&records)[0]["values"][0], 10_000.0);
    assert!(rows(&records)[0]["values"][1].is_null());
    for search in ["/old", "text:/old", "q:/old", "path:/old"] {
        let mut request = snapshot_request("os_cgroup_v2_cpu", &["cgroup_path"]);
        request.search = Some(search.to_owned());
        let records = snapshot_records(&old_only, request);
        assert_eq!(rows(&records).len(), 1);
        assert_eq!(rows(&records)[0]["type_id"], "1201001");
        assert_eq!(rows(&records)[0]["values"][0], "/old");
    }
}

#[test]
fn cgroup_alias_validates_fields_and_labels_against_both_layout_families() {
    let old_only = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/old").expect("path").get());
        buffers.push(legacy_cpu(200, path, 1)).expect("old");
    });
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("id"),
        old_only.as_ref().to_vec(),
        u64::try_from(old_only.len()).expect("len"),
    )
    .expect("embedded source");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    let mut request = snapshot_request("os_cgroup_v2_cpu", &[]);
    request.filters = vec![Filter {
        column: "nr_periods".to_owned(),
        value: "1".to_owned(),
    }];
    assert!(
        matches!(execute(&context, QueryRequest::Snapshot(request.clone())), Err(crate::QueryError::BadFilter(field)) if field == "nr_periods")
    );
    request.filters.clear();
    request.fields = vec!["not_a_field".to_owned()];
    assert!(
        matches!(execute(&context, QueryRequest::Snapshot(request.clone())), Err(crate::QueryError::NoSuchColumn(field)) if field == "not_a_field")
    );
    request.fields.clear();
    request.filters = vec![Filter {
        column: "cgroup_identity".to_owned(),
        value: "new-only-identity".to_owned(),
    }];
    request.page_size = Some(1);
    let records = snapshot_records(&old_only, request);
    assert!(rows(&records).is_empty());
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("empty page");
    assert_eq!(page["eligible"], "0");

    let new_only = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/new").expect("path").get());
        buffers
            .push(discovered_cpu(200, path, path, None))
            .expect("new");
    });
    let records = snapshot_records(&new_only, snapshot_request("os_cgroup_v2_cpu", &["scope"]));
    assert!(
        rows(&records)[0]["values"][0].is_null(),
        "legacy-only valid field remains unavailable in new layout"
    );
}

#[test]
fn cgroup_interval_deltas_keep_null_reset_zero_and_pid_source_boundaries() {
    use kronika_registry::os_cgroup_v2_memory::OsCgroupV2Memory;
    use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        for (ts, count, source) in [
            (50, None, 1),
            (100, Some(0), 1),
            (150, Some(5), 1),
            (200, Some(1), 1),
            (250, Some(2), 2),
            (300, Some(2), 2),
        ] {
            let mut cpu = discovered_cpu(ts, path, path, count);
            cpu.throttled_usec = count;
            buffers.push(cpu).expect("CPU");
            buffers
                .push(OsCgroupV2Pids {
                    ts: Ts(ts),
                    cgroup_path: path,
                    cgroup_identity: path,
                    current: Some(1),
                    max: Some(10),
                    max_unlimited: Some(false),
                    failure_max: count,
                    events_source: source,
                })
                .expect("PIDs");
            buffers
                .push(OsCgroupV2Memory {
                    ts: Ts(ts),
                    cgroup_path: path,
                    cgroup_identity: path,
                    current: Some(1),
                    max: None,
                    max_unlimited: Some(true),
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
                    local_oom_kill: count,
                    local_oom_group_kill: None,
                })
                .expect("memory");
        }
    });
    for (section, field) in [
        ("os_cgroup_v2_cpu", "throttled_interval"),
        ("os_cgroup_v2_memory", "local_oom_kill_delta"),
        ("os_cgroup_v2_pids", "failure_max_delta"),
    ] {
        for (at, expected) in [
            (50, Value::Null),
            (100, Value::Null),
            (150, json!("5")),
            (200, Value::Null),
            (300, json!("0")),
        ] {
            let mut request = snapshot_request(section, &[field]);
            request.at = at;
            request.page_size = Some(1);
            request.by = vec![format!("derived.{field}")];
            let records = snapshot_records(&payload, request);
            assert_eq!(
                rows(&records)[0]["values"][0],
                expected,
                "{section} at {at}"
            );
        }
    }
    let mut request = snapshot_request("os_cgroup_v2_pids", &["failure_max_delta"]);
    request.at = 250;
    let records = snapshot_records(&payload, request);
    assert!(
        rows(&records)[0]["values"][0].is_null(),
        "changed events interface is a different counter history"
    );
}

#[test]
fn cgroup_family_is_selected_before_valid_missing_label_filter() {
    use kronika_registry::os_cgroup_pids::OsCgroupPids;
    use kronika_registry::os_cgroup_v2_pids::OsCgroupV2Pids;
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        buffers
            .push(OsCgroupV2Pids {
                ts: Ts(100),
                cgroup_path: path,
                cgroup_identity: path,
                current: Some(3),
                max: None,
                max_unlimited: Some(true),
                failure_max: Some(5),
                events_source: 1,
            })
            .expect("new older observation");
        buffers
            .push(OsCgroupPids {
                ts: Ts(200),
                cgroup_path: path,
                current: 4,
                max: None,
                scope: 1,
            })
            .expect("legacy newer observation without events source");
    });
    for page_size in [None, Some(1)] {
        let mut request = snapshot_request("os_cgroup_v2_pids", &["current", "events_source"]);
        request.page_size = page_size;
        request.filters.push(Filter {
            column: "events_source".to_owned(),
            value: "1".to_owned(),
        });
        request.at = 100;
        let before = snapshot_records(&payload, request.clone());
        assert_eq!(rows(&before).len(), 1);
        assert_eq!(rows(&before)[0]["type_id"], "1209001");
        request.at = 200;
        let after = snapshot_records(&payload, request);
        assert!(
            rows(&after).is_empty(),
            "newer nonmatching family must not revive the old observation"
        );
        if page_size.is_some() {
            let page = after
                .iter()
                .find(|record| record["record"] == "snapshot_page")
                .expect("page");
            assert_eq!(page["eligible"], "0");
            assert_eq!(page["to"], "200");
        }
    }
}

#[test]
fn cgroup_interval_sort_orders_actual_deltas_across_different_elapsed_intervals() {
    use kronika_registry::os_cgroup_cpu::OsCgroupCpuV3;
    let payload = fixture_payload(|interner, buffers| {
        let path_a = StrId(interner.intern(b"/long-interval").expect("path").get());
        let path_b = StrId(interner.intern(b"/short-interval").expect("path").get());
        for (ts, value) in [(50, 0), (200, 60)] {
            let mut cpu = legacy_cpu(ts, path_a, 0);
            cpu.throttled_usec = value;
            buffers.push(cpu).expect("long interval CPU");
        }
        for (ts, value) in [(100, 0), (200, 50)] {
            buffers
                .push(OsCgroupCpuV3 {
                    ts: Ts(ts),
                    cgroup_path: path_b,
                    cgroup_identity: path_b,
                    usage_usec: 0,
                    user_usec: 0,
                    system_usec: 0,
                    throttled_usec: Some(value),
                    nr_throttled: Some(0),
                    quota_usec: Some(-1),
                    period_usec: Some(100_000),
                    scope: 1,
                })
                .expect("short interval CPU");
        }
    });
    let mut request = snapshot_request(
        "os_cgroup_v2_cpu",
        &["cgroup_path", "throttled_interval", "throttled_usec"],
    );
    request.page_size = Some(1);
    request.by = vec!["derived.throttled_interval".to_owned()];
    let first = snapshot_records(&payload, request.clone());
    assert_eq!(rows(&first)[0]["values"][0], "/long-interval");
    assert_eq!(rows(&first)[0]["values"][1], "60");
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("page");
    request.cursor = page["next_cursor"].as_str().map(str::to_owned);
    let second = snapshot_records(&payload, request.clone());
    assert_eq!(rows(&second)[0]["values"][0], "/short-interval");
    assert_eq!(rows(&second)[0]["values"][1], "50");
    request.cursor = None;
    request.by = vec!["throttled_usec".to_owned()];
    let rate_order = snapshot_records(&payload, request);
    assert_eq!(
        rows(&rate_order)[0]["values"][0],
        "/short-interval",
        "rate order differs from interval order"
    );
}

#[test]
fn shared_projection_preserves_old_only_cgroup_alias_in_both_request_orders() {
    let payload = fixture_payload(|interner, buffers| {
        let path = StrId(interner.intern(b"/work").expect("path").get());
        buffers.push(legacy_cpu(100, path, 1)).expect("old before");
        buffers.push(legacy_cpu(200, path, 2)).expect("old after");
    });
    for reverse in [false, true] {
        for fields in [
            vec!["usage_usec"],
            vec!["usage_usec", "nr_periods"],
            vec!["nr_periods"],
        ] {
            let mut request = snapshot_request("os_cgroup_cpu", &fields);
            request.sections.push("os_cgroup_v2_cpu".to_owned());
            if reverse {
                request.sections.reverse();
            }
            let records = snapshot_records(&payload, request);
            let mut current = String::new();
            let mut values = BTreeMap::new();
            for record in records {
                if record["record"] == "layout" {
                    record["layout"]["logical_name"]
                        .as_str()
                        .expect("logical section")
                        .clone_into(&mut current);
                } else if record["record"] == "row" {
                    assert_eq!(record["type_id"], "1201001");
                    values.insert(current.clone(), record["values"].clone());
                }
            }
            let expected = fields
                .iter()
                .map(|field| {
                    if *field == "usage_usec" {
                        json!(10_000.0)
                    } else {
                        Value::Null
                    }
                })
                .collect::<Vec<_>>();
            assert_eq!(
                values.get("os_cgroup_v2_cpu"),
                Some(&json!(expected)),
                "alias fields retained with order reverse={reverse}"
            );
            if fields.contains(&"usage_usec") {
                assert_eq!(values.get("os_cgroup_cpu"), Some(&json!([10_000.0])));
            }
        }
    }
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("id"),
        payload.to_vec(),
        u64::try_from(payload.len()).expect("length"),
    )
    .expect("source");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    let mut request = snapshot_request("os_cgroup_cpu", &["not_a_field"]);
    request.sections.push("os_cgroup_v2_cpu".to_owned());
    assert!(
        matches!(execute(&context, QueryRequest::Snapshot(request)), Err(crate::QueryError::NoSuchColumn(field)) if field == "not_a_field")
    );
}

#[test]
fn shared_cgroup_virtual_projection_uses_own_quota_in_both_family_orders() {
    for new_recorded in [false, true] {
        let payload = fixture_payload(|interner, buffers| {
            let path = StrId(interner.intern(b"/work").expect("path").get());
            for ts in [100, 200] {
                buffers.push(legacy_cpu(ts, path, ts)).expect("legacy");
                if new_recorded {
                    let mut cpu = discovered_cpu(ts, path, path, Some(ts));
                    cpu.cpuset_cpus = None;
                    buffers.push(cpu).expect("new");
                }
            }
        });
        for reverse in [false, true] {
            for fields in [
                vec!["quota_cores"],
                vec!["quota_cores", "cpuset_cpus", "usage_usec"],
            ] {
                let mut request = snapshot_request("os_cgroup_cpu", &fields);
                request.sections.push("os_cgroup_v2_cpu".to_owned());
                if reverse {
                    request.sections.reverse();
                }
                let records = snapshot_records(&payload, request);
                let mut current = String::new();
                let mut selected = Vec::new();
                for record in records {
                    if record["record"] == "layout" {
                        record["layout"]["logical_name"]
                            .as_str()
                            .expect("section")
                            .clone_into(&mut current);
                    } else if record["record"] == "row" && current == "os_cgroup_v2_cpu" {
                        selected.push(record);
                    }
                }
                assert_eq!(selected.len(), 1);
                assert_eq!(
                    selected[0]["type_id"],
                    if new_recorded { "1207001" } else { "1201001" }
                );
                assert_eq!(
                    selected[0]["values"][0],
                    if new_recorded { json!(1.5) } else { json!(2.0) }
                );
                if fields.len() > 1 {
                    assert!(selected[0]["values"][1].is_null());
                    assert_eq!(selected[0]["values"][2], 1_000_000.0);
                }
            }
        }
        let source = EmbeddedSource::from_owned(
            SegmentId::new(SEGMENT_ID).expect("id"),
            payload.to_vec(),
            u64::try_from(payload.len()).expect("length"),
        )
        .expect("source");
        let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
        let mut request = snapshot_request("os_cgroup_cpu", &["quota_cores", "unknown_virtual"]);
        request.sections.push("os_cgroup_v2_cpu".to_owned());
        assert!(
            matches!(execute(&context, QueryRequest::Snapshot(request)), Err(crate::QueryError::NoSuchColumn(field)) if field == "unknown_virtual")
        );
    }
}
