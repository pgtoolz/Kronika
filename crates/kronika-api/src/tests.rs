use super::{
    DEFAULT_SNAPSHOT_PAGE_SIZE, HeatmapRequest, MAX_SEARCH_EXPRESSION_CHARS,
    MAX_SNAPSHOT_PAGE_SIZE, Route, RouteError, parse,
};
use kronika_query::{
    ActiveCursor, CatalogRequest, DataRequest, EventsRepresentation, Filter, HourPart,
    IndexRequest, Order, SegmentRequest, StatementScope, Window,
};

#[test]
fn catalog_accepts_only_valid_ordered_bounds() {
    assert_eq!(
        parse("/api/catalog", Some("from=-5&to=20")),
        Ok(Route::Catalog(CatalogRequest {
            window: Window {
                from: Some(-5),
                to: Some(20),
            },
        }))
    );
    assert_eq!(
        parse("/api/catalog", Some("from=20&to=5")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
    assert_eq!(
        parse("/api/catalog", Some("colour=green")),
        Err(RouteError::BadParameter("colour".to_owned()))
    );
}

#[test]
fn events_route_is_half_open_deduplicated_and_strict() {
    let Route::Events(request) = parse(
        "/api/events",
        Some("from=10&to=20&representation=occurrences&limit=5&source=pg_log_errors&source=pg_log_temp_files&source=pg_log_errors"),
    )
    .expect("events route") else {
        panic!("events route");
    };
    assert_eq!(request.range().from(), 10);
    assert_eq!(request.range().to_exclusive(), 20);
    assert_eq!(request.representation(), EventsRepresentation::Occurrences);
    assert_eq!(
        request.sources().collect::<Vec<_>>(),
        ["pg_log_errors", "pg_log_temp_files"]
    );

    for query in [
        "to=20&limit=5",
        "from=10&limit=5",
        "from=20&to=10&limit=5",
        "from=0&to=3600000001&limit=5",
        "from=10&to=20&limit=0",
        "from=10&to=20&limit=5&representation=raw",
        "from=10&to=20&limit=5&source=pg_log_temp_files",
    ] {
        assert!(parse("/api/events", Some(query)).is_err(), "{query}");
    }
}

#[test]
fn resources_name_a_segment_and_textual_section_in_the_path() {
    assert_eq!(
        parse(
            "/api/segments/1700000000000000/sections/pg_stat_statements/index",
            None,
        ),
        Ok(Route::Index(IndexRequest {
            segment_id: 1_700_000_000_000_000,
            section: "pg_stat_statements".to_owned(),
        }))
    );
}

#[test]
fn repeated_fields_and_exact_where_parameters_keep_request_order() {
    let route = parse(
        "/api/segments/7/sections/pg_store_plans/history",
        Some("field=ts&field=planid&where.dbid=4&where.queryid=9"),
    )
    .expect("history");
    assert_eq!(
        route,
        Route::History(DataRequest {
            segment: SegmentRequest {
                segment_id: 7,
                section: "pg_store_plans".to_owned(),
            },
            fields: vec!["ts".to_owned(), "planid".to_owned()],
            filters: vec![
                Filter {
                    column: "dbid".to_owned(),
                    value: "4".to_owned(),
                },
                Filter {
                    column: "queryid".to_owned(),
                    value: "9".to_owned(),
                },
            ],
            type_id: None,
            after: None,
        })
    );
}

#[test]
fn physical_layout_selection_is_available_to_every_generic_row_resource() {
    let Route::History(history) = parse(
        "/api/segments/7/sections/pg_stat_statements/history",
        Some("field=calls&type_id=1002001"),
    )
    .expect("typed history") else {
        panic!("history route");
    };
    assert_eq!(history.type_id, Some(1_002_001));

    let Route::Rows(rows) = parse(
        "/api/segments/7/sections/pg_store_plans/rows",
        Some("field=plan&type_id=1004001"),
    )
    .expect("typed rows") else {
        panic!("rows route");
    };
    assert_eq!(rows.data.type_id, Some(1_004_001));

    let Route::Hour(hour) = parse(
        "/api/hour",
        Some("from=1&to=2&section=pg_stat_statements&field=calls&type_id=1002002"),
    )
    .expect("typed hour") else {
        panic!("hour route");
    };
    assert_eq!(
        hour.series.and_then(|series| series.type_id),
        Some(1_002_002)
    );
}

#[test]
fn hour_parts_pin_one_exact_segment_set_and_active_prefix() {
    let Route::Hour(base) = parse("/api/hour", Some("from=1&to=2&part=base")).expect("hour base")
    else {
        panic!("hour route");
    };
    assert_eq!(base.part, HourPart::Base);
    assert_eq!(base.segments, None);
    assert_eq!(base.active, None);

    let Route::Hour(lanes) = parse(
        "/api/hour",
        Some("from=1&to=2&part=lanes&segments=7%2C9&active=9%2C100"),
    )
    .expect("pinned hour lanes") else {
        panic!("hour route");
    };
    assert_eq!(lanes.part, HourPart::Lanes);
    assert_eq!(lanes.segments, Some(vec![7, 9]));
    assert_eq!(
        lanes.active,
        Some(ActiveCursor {
            segment_id: 9,
            wal_position: 100,
        })
    );

    for query in [
        "from=1&to=2&part=lanes",
        "part=lanes&segments=",
        "from=1&part=lanes&segments=",
        "from=1&to=2&part=base&segments=7",
        "from=1&to=2&part=base&active=7,100",
        "from=1&to=2&part=lanes&segments=7,7",
        "from=1&to=2&part=combined",
        "from=1&to=2&section=os_cpu&part=base",
    ] {
        assert!(parse("/api/hour", Some(query)).is_err(), "{query}");
    }
}

#[test]
fn relation_hour_series_accepts_only_one_exact_aggregate_scope() {
    let Route::Hour(hour) = parse(
        "/api/hour",
        Some(
            "from=1&to=2&section=pg_stat_user_tables&group=schema&field=seq_scan&field=buffer_hit_pct&where.datid=7&where.schemaname=public",
        ),
    )
    .expect("schema relation series")
    else {
        panic!("hour route");
    };
    let series = hour.series.expect("relation series");
    assert_eq!(series.group, Some(super::RelationGroup::Schema));
    assert_eq!(series.fields, ["seq_scan", "buffer_hit_pct"]);
    assert_eq!(
        series.filters,
        [
            Filter {
                column: "datid".to_owned(),
                value: "7".to_owned(),
            },
            Filter {
                column: "schemaname".to_owned(),
                value: "public".to_owned(),
            },
        ]
    );

    let Route::Hour(hour) = parse(
        "/api/hour",
        Some(
            "from=1&to=2&section=pg_stat_user_indexes&group=tablespace&field=main_fork_bytes&where.tablespace_oid=4294967295",
        ),
    )
    .expect("tablespace relation series")
    else {
        panic!("hour route");
    };
    let series = hour.series.expect("relation series");
    assert_eq!(series.group, Some(super::RelationGroup::Tablespace));
    assert_eq!(series.filters[0].column, "tablespace_oid");
    assert_eq!(series.filters[0].value, "4294967295");

    for query in [
        "from=1&to=2&section=pg_stat_user_tables&group=object&field=seq_scan&where.datid=7",
        "from=1&to=2&section=pg_stat_activity&group=database&field=active&where.datid=7",
        "from=1&to=2&section=pg_stat_user_tables&group=database&field=seq_scan",
        "from=1&to=2&section=pg_stat_user_tables&group=schema&field=seq_scan&where.datid=7",
        "from=1&to=2&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=7&type_id=1013005",
        "from=1&to=2&section=pg_stat_user_tables&group=tablespace&field=seq_scan&where.tablespace_oid=0",
        "from=1&to=2&section=pg_stat_user_tables&group=tablespace&field=seq_scan&where.tablespace_oid=4294967296",
    ] {
        assert!(
            parse("/api/hour", Some(query)).is_err(),
            "invalid aggregate series shape must be rejected: {query}",
        );
    }
}

#[test]
fn snapshot_paging_inputs_enable_one_bounded_page() {
    let Route::Snapshot(ordered) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=total_time&field=total_exec_time&field=calls&by=total_time&by=total_exec_time&by=calls"),
    )
    .expect("candidate order") else {
        panic!("snapshot route");
    };
    assert_eq!(ordered.by, ["total_time", "total_exec_time", "calls"]);
    assert_eq!(ordered.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));
    assert_eq!(ordered.direction, Order::Desc);

    let Route::Snapshot(ascending) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_indexes&by=idx_scan&direction=asc"),
    )
    .expect("ascending snapshot") else {
        panic!("snapshot route");
    };
    assert_eq!(ascending.direction, Order::Asc);

    let Route::Snapshot(grouped) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_user_tables&group=schema"),
    )
    .expect("relation group") else {
        panic!("snapshot route");
    };
    assert_eq!(grouped.group, Some(super::RelationGroup::Schema));
    assert_eq!(grouped.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));
    for query in [
        "at=9&section=pg_stat_user_indexes&direction=sideways",
        "at=9&section=pg_stat_user_indexes&direction=asc&direction=desc",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("direction".to_owned())),
        );
    }
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_activity&group=database"),
        ),
        Err(RouteError::BadParameter("group".to_owned())),
    );
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_user_tables&group=object&type_id=1013008"),
        ),
        Err(RouteError::BadParameter("type_id".to_owned())),
    );
    assert_eq!(
        parse(
            "/api/segments/7/snapshot",
            Some("at=9&section=pg_stat_user_tables&section=pg_stat_user_indexes&group=database"),
        ),
        Err(RouteError::BadParameter("section".to_owned())),
    );

    let Route::Snapshot(searched) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=query&search=++slow+query++"),
    )
    .expect("searched page") else {
        panic!("snapshot route");
    };
    assert_eq!(searched.search.as_deref(), Some("slow query"));
    assert_eq!(searched.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));

    let Route::Snapshot(resumed) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&cursor=7%2C0%2C2%2C91%2C101"),
    )
    .expect("resumed page") else {
        panic!("snapshot route");
    };
    assert_eq!(resumed.cursor.as_deref(), Some("7,0,2,91,101"));
    assert_eq!(resumed.page_size, Some(DEFAULT_SNAPSHOT_PAGE_SIZE));

    let Route::Snapshot(sized) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&page_size=17"),
    )
    .expect("explicit page size") else {
        panic!("snapshot route");
    };
    assert_eq!(sized.page_size, Some(17));
}

#[test]
fn statement_text_first_match_requires_one_exact_bounded_shape() {
    let path = "/api/segments/7/snapshot";
    let valid = "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A-42&first_match=1";
    let Route::Snapshot(snapshot) = parse(path, Some(valid)).expect("first Statement text") else {
        panic!("snapshot route");
    };
    assert!(snapshot.first_match);
    assert_eq!(snapshot.page_size, Some(1));
    assert_eq!(snapshot.search.as_deref(), Some("query_id:-42"));

    for query in [
        "at=9&section=pg_stat_statements&field=query&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=queryid&field=query&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_store_plans&field=query&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&by=queryid&page_size=1&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&page_size=1&cursor=x&search=query_id%3A42&first_match=1",
        "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=0",
        "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=1&first_match=1",
    ] {
        assert_eq!(
            parse(path, Some(query)),
            Err(RouteError::BadParameter("first_match".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_accepts_one_unpaged_exact_locator() {
    let Route::Snapshot(locator) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=query&type_id=1002001&row_ordinal=18446744073709551615"),
    )
    .expect("exact locator") else {
        panic!("snapshot route");
    };
    assert_eq!(locator.type_id, Some(1_002_001));
    assert_eq!(locator.row_ordinal, Some(u64::MAX));
    assert_eq!(locator.page_size, None);

    for query in [
        "at=9&section=pg_stat_statements&row_ordinal=1",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&by=queryid",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&search=slow",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&cursor=7,0,0,1,2",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&page_size=1",
        "at=9&section=pg_stat_statements&type_id=1002001&row_ordinal=1&where.queryid=2",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("row_ordinal".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_text_limit_keeps_its_exact_cross_width_value() {
    let Route::Snapshot(snapshot) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=os_cpu&field=user&page_size=1&text=5000000000"),
    )
    .expect("large portable text limit") else {
        panic!("snapshot route");
    };
    assert_eq!(snapshot.text, Some(5_000_000_000));
}

#[test]
fn snapshot_page_size_is_positive_and_bounded() {
    let path = "/api/segments/7/snapshot";
    for page_size in [1, MAX_SNAPSHOT_PAGE_SIZE] {
        let query = format!("at=9&section=pg_stat_statements&page_size={page_size}");
        let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("bounded page size")
        else {
            panic!("snapshot route");
        };
        assert_eq!(snapshot.page_size, Some(page_size));
    }

    for page_size in [0, MAX_SNAPSHOT_PAGE_SIZE + 1] {
        let query = format!("at=9&section=pg_stat_statements&page_size={page_size}");
        assert_eq!(
            parse(path, Some(&query)),
            Err(RouteError::BadParameter("page_size".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_search_is_single_trimmed_and_bounded() {
    let path = "/api/segments/7/snapshot";
    let prefix = "at=9&section=pg_stat_statements&field=query";
    let unicode_boundary = "Ж".repeat(MAX_SEARCH_EXPRESSION_CHARS);
    let query = format!("{prefix}&search=++{unicode_boundary}++");
    let Route::Snapshot(snapshot) = parse(path, Some(&query)).expect("Unicode scalar boundary")
    else {
        panic!("snapshot route");
    };
    assert_eq!(snapshot.search, Some(unicode_boundary));

    let repeated = format!("{prefix}&search=first&search=second");
    let too_long = "Ж".repeat(MAX_SEARCH_EXPRESSION_CHARS + 1);
    for query in [
        format!("{prefix}&search="),
        format!("{prefix}&search=+++"),
        format!("{prefix}&search={too_long}"),
        repeated,
    ] {
        assert_eq!(
            parse(path, Some(&query)),
            Err(RouteError::BadParameter("search".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn snapshot_shares_only_a_projection_between_sections() {
    let Route::Snapshot(projected) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=os_cpu&section=os_meminfo&field=user&field=mem_total"),
    )
    .expect("shared projection") else {
        panic!("snapshot route");
    };
    assert_eq!(projected.sections, ["os_cpu", "os_meminfo"]);
    assert_eq!(projected.fields, ["user", "mem_total"]);
    assert_eq!(projected.page_size, None);
    assert_eq!(projected.cursor, None);
    assert!(projected.search.is_none());
    assert!(projected.by.is_empty());

    let Route::Snapshot(filtered) = parse(
        "/api/segments/7/snapshot",
        Some("at=9&section=pg_stat_statements&field=queryid&where.userid=4"),
    )
    .expect("ordinary filtered snapshot") else {
        panic!("snapshot route");
    };
    assert_eq!(filtered.page_size, None);
    assert_eq!(
        filtered.filters,
        [Filter {
            column: "userid".to_owned(),
            value: "4".to_owned(),
        }]
    );

    for query in [
        "at=9&section=os_cpu&section=os_meminfo&by=user",
        "at=9&section=os_cpu&section=os_meminfo&search=cpu",
        "at=9&section=os_cpu&section=os_meminfo&cursor=7,0,0,1,2",
        "at=9&section=os_cpu&section=os_meminfo&page_size=1",
        "at=9&section=os_cpu&section=os_meminfo&where.cpu_id=-1",
        "at=9&section=os_cpu&section=os_meminfo&type_id=1102001",
    ] {
        assert_eq!(
            parse("/api/segments/7/snapshot", Some(query)),
            Err(RouteError::BadParameter("section".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn active_tail_is_one_strict_physical_cursor() {
    let Route::History(request) = parse(
        "/api/segments/7/sections/os_diskstats/history",
        Some("field=reads&after=7%2C18446744073709551615"),
    )
    .expect("active tail") else {
        panic!("history route");
    };
    assert_eq!(
        request.after,
        Some(ActiveCursor {
            segment_id: 7,
            wal_position: u64::MAX,
        })
    );

    for query in [
        "after=8,1",
        "after=7",
        "after=7,1,2",
        "after=7,-1",
        "after=7,1&after=7,2",
    ] {
        assert_eq!(
            parse("/api/segments/7/sections/os_diskstats/history", Some(query),),
            Err(RouteError::BadParameter("after".to_owned())),
            "{query}",
        );
    }
}

#[test]
fn rows_enforces_order_and_page_bounds() {
    let Route::Rows(request) = parse(
        "/api/segments/7/sections/os_process/rows",
        Some("order=desc&page_size=1000&cursor=7%2C0%2C0%2C50%2C99"),
    )
    .expect("rows") else {
        panic!("rows route");
    };
    assert_eq!(request.order, Order::Desc);
    assert_eq!(request.page_size, 1_000);
    assert_eq!(request.cursor.as_deref(), Some("7,0,0,50,99"));
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_process/rows",
            Some("page_size=0")
        ),
        Err(RouteError::BadParameter("page_size".to_owned()))
    );
}

#[test]
fn a_tail_and_page_cursor_remain_separate_physical_inputs() {
    let Route::Rows(request) = parse(
        "/api/segments/7/sections/os_process/rows",
        Some("after=7,100&cursor=7,200,0,1,9"),
    )
    .expect("page inside an active tail") else {
        panic!("rows route");
    };
    assert_eq!(request.data.after.unwrap().wal_position, 100);
    assert_eq!(request.cursor.as_deref(), Some("7,200,0,1,9"));
}

#[test]
fn malformed_escapes_utf8_duplicates_and_old_routes_are_refused() {
    assert_eq!(
        parse("/api/catalog", Some("from=1&from=2")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
    assert_eq!(
        parse("/api/segments/7/sections/os_cpu/history", Some("field=%ZZ")),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(parse("/api/health", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/top", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/series", None), Err(RouteError::NoSuchPath));
    assert_eq!(parse("/api/rows", None), Err(RouteError::NoSuchPath));
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_cpu/history",
            Some("field=ts&field=ts"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/segments/7/sections/os_cpu/history",
            Some("where.cpu_id=1&where.cpu_id=2"),
        ),
        Err(RouteError::BadParameter("where".to_owned()))
    );
}

#[test]
fn only_the_approved_resource_path_shape_is_recognized() {
    for path in [
        "/api/segments/7/index/os_cpu",
        "/api/segments/7/os_cpu/index",
        "/api/segments/7/section/os_cpu/index",
        "/api/segments/7/sections/os_cpu/index/",
        "/api/segments/7/sections/os_cpu",
    ] {
        assert_eq!(parse(path, None), Err(RouteError::NoSuchPath), "{path}");
    }

    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/index", None),
        Ok(Route::Index(_))
    ));
    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/history", None),
        Ok(Route::History(_))
    ));
    assert!(matches!(
        parse("/api/segments/7/sections/os_cpu/rows", None),
        Ok(Route::Rows(_))
    ));
    assert_eq!(
        parse("/api/segments/7/sections/os_cpu/index", Some("field=ts")),
        Err(RouteError::BadParameter("query".to_owned()))
    );
}

#[test]
fn a_section_is_one_strict_percent_decoded_path_component() {
    assert_eq!(
        parse(
            "/api/segments/7/sections/pg%5Fstat%5Fstatements/index",
            None,
        ),
        Ok(Route::Index(IndexRequest {
            segment_id: 7,
            section: "pg_stat_statements".to_owned(),
        }))
    );
    assert_eq!(
        parse("/api/segments/7/sections/%FF/index", None),
        Err(RouteError::BadParameter("section".to_owned()))
    );
}

#[test]
fn a_grouped_heatmap_rejects_labels() {
    match parse(
        "/api/heatmap",
        Some("from=0&to=1&section=os_process&field=utime&group=comm"),
    ) {
        Ok(Route::Heatmap(request)) => assert_eq!(request.group, vec!["comm".to_owned()]),
        other => panic!("expected a heatmap request, got {other:?}"),
    }
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=os_process&field=utime&group=comm&label=cmdline"),
        ),
        Err(RouteError::BadParameter("label".to_owned()))
    );
}

#[test]
fn a_heatmap_cut_may_sum_several_fields() {
    let parsed = parse(
        "/api/heatmap",
        Some(
            "from=0&to=1&section=pg_stat_user_tables&field=n_tup_ins&field=n_tup_upd&field=n_tup_del",
        ),
    );
    match parsed {
        Ok(Route::Heatmap(request)) => assert_eq!(
            request.fields,
            vec![
                "n_tup_ins".to_owned(),
                "n_tup_upd".to_owned(),
                "n_tup_del".to_owned()
            ]
        ),
        other => panic!("expected a heatmap request, got {other:?}"),
    }
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=a&field=a"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=a&field=b&field=c&field=d&field=e"),
        ),
        Err(RouteError::BadParameter("field".to_owned()))
    );
}

#[test]
fn a_heatmap_request_needs_a_window_a_section_and_one_field() {
    assert_eq!(
        parse(
            "/api/heatmap",
            Some(
                "from=0&to=3599999999&section=pg_stat_statements&field=wal_bytes&columns=60&top=25"
            ),
        ),
        Ok(Route::Heatmap(HeatmapRequest {
            from: 0,
            to: 3_599_999_999,
            section: "pg_stat_statements".to_owned(),
            fields: vec!["wal_bytes".to_owned()],
            columns: 60,
            top: 25,
            group: Vec::new(),
            type_id: None,
            scope: StatementScope::All,
        }))
    );
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=f&label=name")
        ),
        Err(RouteError::BadParameter("label".to_owned()))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=0&to=1&section=s")),
        Err(RouteError::BadParameter("field".to_owned()))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=0&to=1&section=s&field=f&top=0")),
        Err(RouteError::BadParameter("top".to_owned()))
    );
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=0&to=1&section=s&field=f&columns=0")
        ),
        Err(RouteError::BadParameter("columns".to_owned()))
    );
    assert_eq!(
        parse("/api/heatmap", Some("from=2&to=1&section=s&field=f")),
        Err(RouteError::BadParameter("from".to_owned()))
    );
}

#[test]
fn row_detail_accepts_one_bounded_opaque_reference() {
    assert_eq!(
        parse("/api/row-detail", Some("detail_ref=opaque_-ref")),
        Ok(Route::RowDetail("opaque_-ref".to_owned()))
    );
    for query in ["", "detail_ref=", "detail_ref=one&detail_ref=two"] {
        assert_eq!(
            parse("/api/row-detail", Some(query)),
            Err(RouteError::BadParameter("detail_ref".to_owned())),
            "{query}",
        );
    }
    assert_eq!(
        parse("/api/row-detail", Some("detail_ref=one&segment_id=7")),
        Err(RouteError::BadParameter("segment_id".to_owned()))
    );
    let oversized = format!(
        "detail_ref={}",
        "A".repeat(kronika_query::DETAIL_REF_MAX_ENCODED_BYTES + 1)
    );
    assert_eq!(
        parse("/api/row-detail", Some(&oversized)),
        Err(RouteError::BadParameter("detail_ref".to_owned()))
    );
}

#[test]
fn statement_scope_is_accepted_only_where_statements_can_be_scoped() {
    let Route::Heatmap(scoped) = parse(
        "/api/heatmap",
        Some("from=1&to=2&section=pg_stat_statements&field=total_exec_time&scope=workload"),
    )
    .expect("scoped statement ranking") else {
        panic!("heatmap route");
    };
    assert_eq!(scoped.scope, StatementScope::Workload);
    let Route::Heatmap(unscoped) = parse(
        "/api/heatmap",
        Some("from=1&to=2&section=pg_stat_statements&field=total_exec_time"),
    )
    .expect("default statement ranking") else {
        panic!("heatmap route");
    };
    assert_eq!(unscoped.scope, StatementScope::All);
    assert_eq!(
        parse(
            "/api/heatmap",
            Some("from=1&to=2&section=os_cpu&field=user&scope=bogus"),
        ),
        Err(RouteError::BadParameter("scope".to_owned())),
    );
    // A CPU ranking cannot hide collector statements; the query layer refuses it.
    let Route::Heatmap(cpu) = parse(
        "/api/heatmap",
        Some("from=1&to=2&section=os_cpu&field=user&scope=workload"),
    )
    .expect("scope parses before the section check") else {
        panic!("heatmap route");
    };
    assert!(Route::Heatmap(cpu).into_query().is_err());

    let path = "/api/segments/7/snapshot";
    let Route::Snapshot(page) = parse(
        path,
        Some("at=9&section=pg_stat_statements&field=query&page_size=50&scope=workload"),
    )
    .expect("scoped statement page") else {
        panic!("snapshot route");
    };
    assert_eq!(page.scope, StatementScope::Workload);
    for query in [
        "at=9&section=pg_stat_statements&field=query&scope=workload",
        "at=9&section=os_process&field=pid&page_size=50&scope=workload",
        "at=9&section=pg_stat_statements&field=query&page_size=1&search=query_id%3A42&first_match=1&scope=workload",
        "at=9&section=pg_stat_statements&field=query&page_size=50&scope=all&scope=workload",
        "at=9&section=pg_stat_statements&field=query&page_size=50&scope=Workload",
    ] {
        assert_eq!(
            parse(path, Some(query)),
            Err(RouteError::BadParameter("scope".to_owned())),
            "{query}",
        );
    }

    let Route::Hour(summary) = parse(
        "/api/hour",
        Some("from=1&to=2&section=postgresql_summary&scope=workload"),
    )
    .expect("scoped summary") else {
        panic!("hour route");
    };
    assert_eq!(
        summary.series.expect("summary series").scope,
        StatementScope::Workload
    );
    for query in [
        "from=1&to=2&section=os_cpu&field=user&scope=workload",
        "from=1&to=2&scope=workload",
    ] {
        assert_eq!(
            parse("/api/hour", Some(query)),
            Err(RouteError::BadParameter("scope".to_owned())),
            "{query}",
        );
    }
}
