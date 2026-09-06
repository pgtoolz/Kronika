use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_registry::{PgStatDatabaseV1, PgStatDatabaseV4, Section, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use crate::api::Prepared;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const FIELD: &str = "parallel_workers_launched";

fn database_v1(ts: i64, datid: u32, datname: StrId) -> PgStatDatabaseV1 {
    PgStatDatabaseV1 {
        ts: Ts(ts),
        datid,
        datname: Some(datname),
        numbackends: Some(3),
        xact_commit: 100,
        xact_rollback: 2,
        blks_read: 4_000,
        blks_hit: 90_000,
        tup_returned: 500,
        tup_fetched: 400,
        tup_inserted: 50,
        tup_updated: 30,
        tup_deleted: 10,
        conflicts: 0,
        temp_files: 1,
        temp_bytes: 8_192,
        deadlocks: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        stats_reset: Some(Ts(ts - 5)),
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

fn database_v4(
    ts: i64,
    datid: u32,
    datname: StrId,
    parallel_workers_launched: i64,
) -> PgStatDatabaseV4 {
    PgStatDatabaseV4 {
        ts: Ts(ts),
        datid,
        datname: Some(datname),
        numbackends: Some(3),
        xact_commit: 100,
        xact_rollback: 2,
        blks_read: 4_000,
        blks_hit: 90_000,
        tup_returned: 500,
        tup_fetched: 400,
        tup_inserted: 50,
        tup_updated: 30,
        tup_deleted: 10,
        conflicts: 0,
        temp_files: 1,
        temp_bytes: 8_192,
        deadlocks: 0,
        blk_read_time: 12.5,
        blk_write_time: 3.0,
        stats_reset: Some(Ts(ts - 5)),
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
        checksum_failures: Some(0),
        checksum_last_failure: None,
        session_time: 1_000.0,
        active_time: 250.0,
        idle_in_transaction_time: 50.0,
        sessions: 7,
        sessions_abandoned: 1,
        sessions_fatal: 0,
        sessions_killed: 0,
        parallel_workers_to_launch: 9,
        parallel_workers_launched,
    }
}

fn finished_multi_layout_segment() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary data root");
    let root = DataRoot::open(directory.path()).expect("open data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");
    let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
        .expect("segment address");

    let mut interner =
        Interner::new(DictLimits::new(4_096, 4_096).expect("fixture dictionary limits"));
    let postgres = StrId(interner.intern(b"postgres").expect("intern postgres").get());
    let template = StrId(
        interner
            .intern(b"template1")
            .expect("intern template1")
            .get(),
    );
    let dictionaries = dict::encode(interner.window()).expect("encode fixture dictionary");

    let mut buffers = SectionBuffers::new();
    buffers
        .push(database_v1(100, 42, postgres))
        .expect("V1 row fits");
    buffers
        .push(database_v4(200, 42, postgres, 8))
        .expect("matching V4 row fits");
    buffers
        .push(database_v4(300, 43, template, 99))
        .expect("nonmatching V4 row fits");
    let part = buffers
        .flush(&dictionaries)
        .expect("encode fixture part")
        .expect("fixture part has rows");
    journal
        .append(address.id, &part)
        .expect("append fixture part");
    write_segment(&journal, &writer, address).expect("publish fixture segment");
    directory
}

fn stream(prepared: Prepared) -> Vec<Value> {
    let mut records = Vec::new();
    prepared
        .stream(
            &mut |record| {
                records.push(serde_json::from_slice(&record).expect("JSON record"));
                true
            },
            &|| false,
        )
        .expect("stream history");
    records
}

#[test]
fn textual_filter_and_missing_field_work_across_physical_layouts() {
    let directory = finished_multi_layout_segment();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/sections/pg_stat_database/history?field={FIELD}&where.datname=postgres"
    );
    let (path, query) = target.split_once('?').expect("history query");
    let route = crate::route::parse(path, Some(query)).expect("valid history route");
    let prepared = crate::api::prepare(directory.path(), 0b10, route, None)
        .expect("prepare multi-layout history");
    let records = stream(prepared);

    let v1_type_id = PgStatDatabaseV1::CONTRACT.type_id.get().to_string();
    let v4_type_id = PgStatDatabaseV4::CONTRACT.type_id.get().to_string();
    let v1_layout = records
        .iter()
        .find(|record| record["record"] == "layout" && record["layout"]["type_id"] == v1_type_id)
        .expect("V1 projected layout");
    let v4_layout = records
        .iter()
        .find(|record| record["record"] == "layout" && record["layout"]["type_id"] == v4_type_id)
        .expect("V4 projected layout");
    assert_eq!(
        v1_layout["layout"]["columns"],
        json!([{ "name": FIELD, "available": false }]),
        "the old physical layout advertises the requested field as unavailable"
    );
    assert_eq!(
        v4_layout["layout"]["columns"][0]["available"], true,
        "the new physical layout advertises the requested field"
    );

    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        2,
        "the exact text filter excludes the template1 row"
    );
    let v1_row = rows
        .iter()
        .find(|row| row["type_id"] == v1_type_id)
        .expect("matching V1 row");
    let v4_row = rows
        .iter()
        .find(|row| row["type_id"] == v4_type_id)
        .expect("matching V4 row");
    assert_eq!(v1_row["timestamp"], "100", "V1 timestamp is retained");
    assert_eq!(v1_row["identity"], json!([42]), "V1 identity is retained");
    assert_eq!(
        v1_row["values"],
        json!([null]),
        "a field absent from V1 is emitted as null"
    );
    assert_eq!(v4_row["timestamp"], "200", "V4 timestamp is retained");
    assert_eq!(v4_row["identity"], json!([42]), "V4 identity is retained");
    assert_eq!(
        v4_row["values"],
        json!(["8"]),
        "the same requested field is rendered from V4"
    );
}

#[test]
fn snapshot_pages_sort_and_count_across_compatible_physical_layouts() {
    let directory = finished_multi_layout_segment();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=300&section=pg_stat_database&field=datid&field={FIELD}&by=datid&page_size=1"
    );
    let (path, query) = base.split_once('?').expect("snapshot query");
    let route = crate::route::parse(path, Some(query)).expect("snapshot route");
    let first =
        stream(crate::api::prepare(directory.path(), 0b10, route, None).expect("first mixed page"));
    let first_row_position = first
        .iter()
        .position(|record| record["record"] == "row")
        .expect("first row");
    assert_eq!(
        first[..first_row_position]
            .iter()
            .filter(|record| record["record"] == "layout")
            .count(),
        2,
        "all compatible layouts precede globally sorted rows"
    );
    let first_row = &first[first_row_position];
    assert_eq!(
        first_row["type_id"],
        PgStatDatabaseV4::CONTRACT.type_id.get().to_string()
    );
    assert_eq!(first_row["values"][0], 43);
    let first_page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("first page trailer");
    assert_eq!(first_page["eligible"], "2");
    assert_eq!(first_page["returned"], "1");
    assert_eq!(first_page["has_more"], true);
    assert_eq!(first_page["truncated"], true);
    assert_eq!(first_page["order_by"], json!(["datid"]));

    let cursor = first_page["next_cursor"].as_str().expect("next cursor");
    let continued = format!("{base}&cursor={cursor}");
    let (path, query) = continued.split_once('?').expect("continued query");
    let route = crate::route::parse(path, Some(query)).expect("continued route");
    let second = stream(
        crate::api::prepare(directory.path(), 0b10, route, None).expect("second mixed page"),
    );
    let second_row = second
        .iter()
        .find(|record| record["record"] == "row")
        .expect("second row");
    assert_eq!(
        second_row["type_id"],
        PgStatDatabaseV1::CONTRACT.type_id.get().to_string()
    );
    assert_eq!(second_row["values"], json!([42, null]));
    let second_page = second
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("second page trailer");
    assert_eq!(second_page["eligible"], "2");
    assert_eq!(second_page["returned"], "1");
    assert_eq!(second_page["has_more"], false);
    assert_eq!(second_page["truncated"], true);
}
