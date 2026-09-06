use super::{
    StatementsRow, StatementsVersion, capability, statements_query, statements_version, to_v1,
    to_v2, to_v4, to_v6,
};
use crate::extension::{ExtensionSchema, InventoryEntry, parse_version};
use crate::test_intern as fake_intern;

fn layout(extversion: &str) -> Option<StatementsVersion> {
    parse_version(extversion).and_then(statements_version)
}

#[derive(Clone, Copy)]
enum StatementAccess {
    Full,
    Hidden,
    MissingReader,
}

fn inventory(extversion: &str, access: StatementAccess) -> InventoryEntry {
    let (reader, full_visibility) = match access {
        StatementAccess::Full => (true, true),
        StatementAccess::Hidden => (true, false),
        StatementAccess::MissingReader => (false, true),
    };
    InventoryEntry {
        name: "pg_stat_statements".to_owned(),
        extversion: extversion.to_owned(),
        schema: ExtensionSchema::new("odd schema"),
        schema_usable: true,
        full_visibility,
        statements_reader: reader,
        store_plans_zero_arg: false,
        store_plans_bool_arg: false,
        store_plans_key_getter: false,
        store_plans_text_converter: false,
        store_plans_ossc_columns: false,
        store_plans_vadv_columns: false,
        store_plans_datasentinel_columns: false,
        statements_info: false,
        store_plans_info: false,
    }
}

fn sample_row() -> StatementsRow {
    StatementsRow {
        ts: 2_000,
        queryid: Some(-42),
        userid: 10,
        dbid: 16_400,
        toplevel: Some(true),
        datname: Some("appdb".to_owned()),
        usename: Some("app".to_owned()),
        query: Some("select 1".to_owned()),
        calls: 1_000,
        rows: 25_000,
        plans: Some(900),
        total_exec_time: 4_200.5,
        total_plan_time: Some(120.0),
        min_exec_time: 0.5,
        max_exec_time: 90.0,
        mean_exec_time: 4.2,
        stddev_exec_time: 1.1,
        min_plan_time: Some(0.1),
        max_plan_time: Some(5.0),
        mean_plan_time: Some(0.13),
        stddev_plan_time: Some(0.02),
        shared_blks_hit: 900_000,
        shared_blks_read: 4_000,
        shared_blks_dirtied: 300,
        shared_blks_written: 100,
        local_blks_hit: 10,
        local_blks_read: 2,
        local_blks_dirtied: 1,
        local_blks_written: 0,
        temp_blks_read: 7,
        temp_blks_written: 9,
        shared_blk_read_time: 33.0,
        shared_blk_write_time: 4.0,
        local_blk_read_time: Some(0.5),
        local_blk_write_time: Some(0.25),
        temp_blk_read_time: Some(2.0),
        temp_blk_write_time: Some(3.0),
        wal_records: Some(50_000),
        wal_fpi: Some(400),
        wal_bytes: Some(9_000_000),
        wal_buffers_full: Some(12),
        jit_functions: Some(6),
        jit_generation_time: Some(1.5),
        jit_inlining_count: Some(2),
        jit_inlining_time: Some(0.75),
        jit_optimization_count: Some(2),
        jit_optimization_time: Some(3.5),
        jit_emission_count: Some(2),
        jit_emission_time: Some(2.25),
        jit_deform_count: Some(4),
        jit_deform_time: Some(0.9),
        parallel_workers_to_launch: Some(8),
        parallel_workers_launched: Some(6),
        stats_since: Some(1_000),
        minmax_stats_since: Some(1_500),
    }
}

#[test]
fn the_extension_version_selects_the_layout() {
    assert_eq!(layout("1.5"), Some(StatementsVersion::V1));
    assert_eq!(layout("1.6"), Some(StatementsVersion::V1));
    assert_eq!(layout("1.7"), Some(StatementsVersion::V1));
    assert_eq!(layout("1.8"), Some(StatementsVersion::V2));
    assert_eq!(layout("1.9"), Some(StatementsVersion::V3));
    assert_eq!(layout("1.10"), Some(StatementsVersion::V4));
    assert_eq!(layout("1.11"), Some(StatementsVersion::V5));
    assert_eq!(layout("1.12"), Some(StatementsVersion::V6));
}

#[test]
fn an_extension_below_the_supported_floor_or_unknown_is_not_collected() {
    assert_eq!(layout("1.4"), None);
    assert_eq!(layout("2.0"), None);
}

#[test]
fn a_later_release_of_the_same_line_keeps_the_newest_layout() {
    assert_eq!(layout("1.13"), Some(StatementsVersion::V6));
}

#[test]
fn a_query_asks_only_for_columns_its_extension_has() {
    let schema = ExtensionSchema::new("public");
    assert!(statements_query(StatementsVersion::V1, &schema).contains("s.total_time"));
    assert!(!statements_query(StatementsVersion::V1, &schema).contains("s.plans"));
    assert!(statements_query(StatementsVersion::V2, &schema).contains("s.total_plan_time"));
    assert!(!statements_query(StatementsVersion::V2, &schema).contains("s.toplevel"));
    assert!(statements_query(StatementsVersion::V3, &schema).contains("s.toplevel"));
    assert!(!statements_query(StatementsVersion::V3, &schema).contains("jit_functions"));
    assert!(statements_query(StatementsVersion::V4, &schema).contains("jit_functions"));
    assert!(!statements_query(StatementsVersion::V4, &schema).contains("jit_deform_count"));
    assert!(statements_query(StatementsVersion::V5, &schema).contains("jit_deform_count"));
    assert!(!statements_query(StatementsVersion::V5, &schema).contains("wal_buffers_full"));
    assert!(statements_query(StatementsVersion::V6, &schema).contains("wal_buffers_full"));
}

#[test]
fn the_block_timing_columns_are_asked_for_under_the_name_that_release_uses() {
    let schema = ExtensionSchema::new("public");
    assert!(statements_query(StatementsVersion::V4, &schema).contains("s.blk_read_time AS"));
    assert!(!statements_query(StatementsVersion::V4, &schema).contains("s.local_blk_read_time"));
    assert!(statements_query(StatementsVersion::V5, &schema).contains("s.shared_blk_read_time"));
    assert!(statements_query(StatementsVersion::V5, &schema).contains("s.local_blk_read_time"));
}

#[test]
fn every_query_carries_the_marker_and_bounds_statement_text() {
    let schema = ExtensionSchema::new("metrics\"schema");
    for version in [
        StatementsVersion::V1,
        StatementsVersion::V2,
        StatementsVersion::V3,
        StatementsVersion::V4,
        StatementsVersion::V5,
        StatementsVersion::V6,
    ] {
        let sql = statements_query(version, &schema);
        assert!(sql.contains("kronika:"), "{sql}");
        assert!(sql.contains("left(s.query, 65536) AS query"), "{sql}");
        assert!(sql.contains("\"metrics\"\"schema\".\"pg_stat_statements\"(true)"));
        assert!(sql.contains("WHERE s.queryid IS NOT NULL"), "{sql}");
    }
}

#[test]
fn statement_collection_requires_full_visibility() {
    assert!(capability(&inventory("1.12", StatementAccess::Full), 18).is_some());
    assert!(capability(&inventory("1.12", StatementAccess::Hidden), 18).is_none());
}

#[test]
fn the_reader_and_schema_must_be_usable() {
    assert!(capability(&inventory("1.12", StatementAccess::MissingReader), 18).is_none());
    let mut inaccessible = inventory("1.12", StatementAccess::Full);
    inaccessible.schema_usable = false;
    assert!(capability(&inaccessible, 18).is_none());
}

#[test]
fn postgres_fourteen_requires_a_toplevel_aware_extension_layout() {
    assert!(capability(&inventory("1.8", StatementAccess::Full), 13).is_some());
    assert!(capability(&inventory("1.8", StatementAccess::Full), 14).is_none());
    assert!(capability(&inventory("1.9", StatementAccess::Full), 14).is_some());
}

#[test]
fn to_v6_maps_every_column_including_bounded_text() {
    let r = to_v6(&sample_row(), fake_intern).expect("infallible intern");
    assert_eq!(r.ts.0, 2_000);
    assert_eq!(r.queryid, Some(-42));
    assert_eq!(r.datname, Some(fake_intern(b"appdb").unwrap()));
    assert_eq!(r.query, Some(fake_intern(b"select 1").unwrap()));
    assert!(r.toplevel);
    assert_eq!(r.calls, 1_000);
    assert!((r.total_exec_time - 4_200.5).abs() < f64::EPSILON);
    assert!((r.shared_blk_read_time - 33.0).abs() < f64::EPSILON);
    assert_eq!(r.wal_buffers_full, 12);
    assert_eq!(r.parallel_workers_launched, 6);
    assert_eq!(r.stats_since.0, 1_000);
}

#[test]
fn a_masked_query_id_stays_absent() {
    let mut row = sample_row();
    row.queryid = None;
    assert_eq!(to_v6(&row, fake_intern).expect("intern").queryid, None);
}

#[test]
fn a_column_the_extension_never_sent_lands_as_zero() {
    let mut row = sample_row();
    row.wal_buffers_full = None;
    row.jit_deform_count = None;
    row.total_plan_time = None;
    let r = to_v6(&row, fake_intern).expect("intern");
    assert_eq!(r.wal_buffers_full, 0);
    assert_eq!(r.jit_deform_count, 0);
    assert!(r.total_plan_time.abs() < f64::EPSILON);
}

#[test]
fn the_older_layouts_keep_the_block_timing_under_its_old_name() {
    let r = to_v4(&sample_row(), fake_intern).expect("intern");
    assert!((r.blk_read_time - 33.0).abs() < f64::EPSILON);
    assert!((r.blk_write_time - 4.0).abs() < f64::EPSILON);
}

#[test]
fn to_v2_has_the_planning_columns_but_no_toplevel() {
    let r = to_v2(&sample_row(), fake_intern).expect("intern");
    assert_eq!(r.plans, 900);
    assert!((r.total_plan_time - 120.0).abs() < f64::EPSILON);
}

#[test]
fn to_v1_maps_execution_time_onto_the_name_that_release_used() {
    let r = to_v1(&sample_row(), fake_intern).expect("intern");
    assert!((r.total_time - 4_200.5).abs() < f64::EPSILON);
    assert!((r.mean_time - 4.2).abs() < f64::EPSILON);
    assert_eq!(r.calls, 1_000);
}

#[test]
fn intern_failure_propagates() {
    assert_eq!(to_v6(&sample_row(), |_| Err("full")), Err("full"));
}
