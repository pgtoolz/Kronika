use crate::StatementScope;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use kronika_format::{DictLimits, ReadAt};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::{Cell, Row};
use kronika_registry::contract;
use kronika_registry::os_process::OsProcess;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::pg_stat_user_tables::PgStatUserTablesV1;
use kronika_registry::{StrId, Ts};
use kronika_store::{EmbeddedSource, ImmutableSegmentSource, PosixSource, ResourceCatalog};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use super::{
    ContributingMoments, GlobPattern, PageOrderValue, PageRankedRow, PageRows, PageStagedRow,
    SearchValue, SnapshotCursor, StructuredSearch, available_field_index, compare_ordered,
    compare_products, context_operations, first_match_rows, ordered_cell, page_operations,
    plan_statement_query_id_columns, prepared_search, rate, record_contributing_moment,
    relation_snapshot_operations, reset_context_operations, reset_first_match_rows,
    reset_page_operations, reset_relation_snapshot_operations, scheduled_ticks, snapshot_binding,
    timed_context_index,
};
use crate::{
    Filter, FinishedDataset, Order, OutputField, QueryContext, QueryDataset, QueryRequest,
    QuerySink, RelationGroup, SnapshotRequest, execute,
};

const COLUMN: &str = "counter";
const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SNAPSHOT_AT: i64 = 200;

#[derive(Default)]
struct SnapshotRecords(Vec<Value>);

impl QuerySink for SnapshotRecords {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0
            .push(serde_json::from_slice(&bytes).expect("snapshot record JSON"));
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn fixture_payload(fill: impl FnOnce(&mut Interner, &mut SectionBuffers)) -> Arc<[u8]> {
    let root = tempfile::tempdir().expect("snapshot fixture directory");
    let data_root = DataRoot::open(root.path()).expect("snapshot fixture data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("snapshot fixture writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("snapshot journal");
    let mut interner = Interner::new(DictLimits::default());
    let mut buffers = SectionBuffers::new();
    fill(&mut interner, &mut buffers);
    let dictionary = dict::encode(interner.window()).expect("snapshot fixture dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode snapshot fixture")
        .expect("nonempty snapshot fixture");
    let segment_id = SegmentId::new(SEGMENT_ID).expect("snapshot segment id");
    journal
        .append(segment_id, &part)
        .expect("append snapshot fixture");
    let address = SegmentAddress::new(segment_id).expect("snapshot segment address");
    write_segment(&journal, &owner, address).expect("finish snapshot fixture");
    journal.reset().expect("reset snapshot fixture journal");
    drop(journal);
    drop(owner);

    let source = PosixSource::open(root.path()).expect("open snapshot fixture source");
    let listing = source.resources().expect("list snapshot fixture resource");
    let resource = listing
        .resources
        .first()
        .expect("snapshot fixture resource");
    let bytes = source
        .open_resource(resource)
        .expect("open snapshot fixture resource");
    let len = usize::try_from(bytes.byte_len().expect("snapshot fixture length"))
        .expect("snapshot fixture length fits memory");
    let mut payload = vec![0; len];
    bytes
        .read_exact_at(&mut payload, 0)
        .expect("read snapshot fixture resource");
    payload.into()
}

fn snapshot_request(section: &str, fields: &[&str]) -> SnapshotRequest {
    SnapshotRequest {
        segment_id: SEGMENT_ID,
        at: SNAPSHOT_AT,
        sections: vec![section.to_owned()],
        fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
        scope: StatementScope::All,
    }
}

fn snapshot_records(payload: &Arc<[u8]>, request: SnapshotRequest) -> Vec<Value> {
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("snapshot segment id"),
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("snapshot payload length fits u64"),
    )
    .expect("embedded snapshot source");
    let dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(source));
    let context = QueryContext::new(dataset, 0b11, false);
    let execution = execute(&context, QueryRequest::Snapshot(request)).expect("prepare snapshot");
    let mut records = SnapshotRecords::default();
    execution
        .stream(&mut records)
        .expect("stream snapshot records");
    records.0
}

fn process(pid: i32, label: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(SNAPSHOT_AT),
        pid,
        starttime: Ts(SEGMENT_ID - 1_000_000 + i64::from(pid)),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'S',
        num_threads: 1,
        tty: 0,
        comm: label,
        cmdline: Some(label),
        utime: 0,
        stime: 0,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 0,
        rundelay_ns: 0,
        blkdelay_ticks: 0,
        nvcsw: 0,
        nivcsw: 0,
        minflt: 0,
        majflt: 0,
        vmem_kb: 0,
        rmem_kb: 0,
        vswap_kb: 0,
        syscr: None,
        syscw: None,
        rchar: None,
        wchar: None,
        read_bytes: None,
        write_bytes: None,
        cancelled_write_bytes: None,
        exit_signal: 17,
        scope: 0,
    }
}

fn statement(dbid: u32, query: Option<StrId>) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(SNAPSHOT_AT),
        queryid: Some(42),
        userid: 72,
        dbid,
        datname: None,
        usename: None,
        query,
        calls: 1,
        rows: 0,
        plans: 0,
        total_exec_time: 1.0,
        total_plan_time: 0.0,
        min_exec_time: 0.0,
        max_exec_time: 0.0,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
        stddev_plan_time: 0.0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        wal_records: 0,
        wal_fpi: 0,
        wal_bytes: 0,
    }
}

fn table(ts: i64, relid: u32, seq_scan: i64, label: StrId) -> PgStatUserTablesV1 {
    PgStatUserTablesV1 {
        ts: Ts(ts),
        datid: 1,
        datname: label,
        relid,
        schemaname: label,
        relname: label,
        tablespace_oid: None,
        tablespace: None,
        seq_scan,
        seq_tup_read: 0,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: 0,
        n_tup_upd: 0,
        n_tup_del: 0,
        n_tup_hot_upd: 0,
        n_live_tup: 0,
        n_dead_tup: 0,
        n_mod_since_analyze: 0,
        vacuum_count: 0,
        autovacuum_count: 0,
        analyze_count: 0,
        autoanalyze_count: 0,
        last_vacuum: None,
        last_autovacuum: None,
        last_analyze: None,
        last_autoanalyze: None,
        main_fork_bytes: 0,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        xid_age: None,
        mxid_age: None,
        reltuples: 0,
        heap_blks_read: 0,
        heap_blks_hit: 0,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
    }
}

#[test]
fn snapshot_top_k_keeps_page_size_plus_one_and_dictionary_work_chunked() {
    const ROWS: i32 = 1_025;
    let payload = fixture_payload(|interner, buffers| {
        let label = StrId(
            interner
                .intern(b"worker")
                .expect("intern process label")
                .get(),
        );
        for pid in 0..ROWS {
            buffers.push(process(pid, label)).expect("process row fits");
        }
    });
    reset_page_operations();
    let mut request = snapshot_request("os_process", &["pid", "comm"]);
    request.by = vec!["comm".to_owned()];
    request.page_size = Some(10);
    let records = snapshot_records(&payload, request);

    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"] == "row")
            .count(),
        10
    );
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("snapshot page trailer");
    assert_eq!(page["eligible"], "1025");
    assert_eq!(page["returned"], "10");
    assert_eq!(page["has_more"], true);
    assert_eq!(page_operations(), (1, 2, 1_024));
}

#[test]
fn snapshot_first_match_stops_after_the_first_nonempty_query_text() {
    let payload = fixture_payload(|interner, buffers| {
        let text = StrId(
            interner
                .intern(b"select 42")
                .expect("intern query text")
                .get(),
        );
        buffers
            .push(statement(73, None))
            .expect("empty statement row fits");
        for dbid in 74..=202 {
            buffers
                .push(statement(dbid, Some(text)))
                .expect("statement row fits");
        }
    });
    reset_first_match_rows();
    reset_page_operations();
    let mut request = snapshot_request("pg_stat_statements", &["query"]);
    request.page_size = Some(1);
    request.search = Some("query_id:42".to_owned());
    request.first_match = true;
    let records = snapshot_records(&payload, request);

    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], json!(["select 42"]));
    assert_eq!(first_match_rows(), 2);
    assert_eq!(page_operations(), (0, 0, 0));
}

#[test]
fn unpaged_snapshot_staging_and_filter_dictionary_stay_chunk_bounded() {
    const ROWS: i32 = 1_025;
    let payload = fixture_payload(|interner, buffers| {
        let label = StrId(
            interner
                .intern(b"worker")
                .expect("intern process label")
                .get(),
        );
        for pid in 0..ROWS {
            buffers.push(process(pid, label)).expect("process row fits");
        }
    });
    reset_context_operations();
    let mut request = snapshot_request("os_process", &["pid"]);
    request.filters = vec![Filter {
        column: "comm".to_owned(),
        value: "worker".to_owned(),
    }];
    let records = snapshot_records(&payload, request);

    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"] == "row")
            .count(),
        usize::try_from(ROWS).expect("positive row count")
    );
    assert_eq!(context_operations(), (1_024, 1_025, 1));
}

#[test]
fn snapshot_preflight_exposes_identity_before_predecessor_scans() {
    let payload = fixture_payload(|interner, buffers| {
        let label = StrId(interner.intern(b"relation").expect("intern label").get());
        buffers
            .push(table(100, 11, 10, label))
            .expect("predecessor row fits");
        buffers
            .push(table(SNAPSHOT_AT, 11, 20, label))
            .expect("current row fits");
    });
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("snapshot segment id"),
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("snapshot payload length fits u64"),
    )
    .expect("embedded snapshot source");
    let context = QueryContext::new(Arc::new(FinishedDataset::new(source)), 0b11, false);
    let mut request = snapshot_request("pg_stat_user_tables", &["seq_scan"]);
    request.group = Some(RelationGroup::Object);

    reset_relation_snapshot_operations();
    let preparation = super::prepare_snapshot(&context, request).expect("snapshot preflight");
    assert!(preparation.metadata().identity().is_some());
    assert_eq!(relation_snapshot_operations(), (0, 0, 0));

    let _execution = preparation.finish().expect("finish snapshot planning");
    assert!(relation_snapshot_operations().0 > 0);
}

#[test]
fn relation_snapshot_bounds_predecessor_scans_and_page_metric_projection() {
    let payload = fixture_payload(|interner, buffers| {
        let label = StrId(
            interner
                .intern(b"relation")
                .expect("intern relation label")
                .get(),
        );
        for (relid, current) in [(11, 30), (12, 15), (13, 60)] {
            buffers
                .push(table(100, relid, 0, label))
                .expect("predecessor relation row fits");
            buffers
                .push(table(SNAPSHOT_AT, relid, current, label))
                .expect("current relation row fits");
        }
    });
    reset_relation_snapshot_operations();
    let mut request = snapshot_request("pg_stat_user_tables", &["seq_scan"]);
    request.group = Some(RelationGroup::Object);
    request.by = vec!["seq_scan".to_owned()];
    request.page_size = Some(1);
    let records = snapshot_records(&payload, request);

    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"] == "relation")
            .count(),
        1
    );
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("relation page trailer");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], true);
    assert_eq!(relation_snapshot_operations(), (1, 1, 1));
}

#[test]
fn plan_statement_query_id_mapping_is_fork_transparent() {
    assert_eq!(plan_statement_query_id_columns(1_003_001), ["queryid"]);
    assert_eq!(plan_statement_query_id_columns(1_018_001), ["queryid"]);
    assert_eq!(
        plan_statement_query_id_columns(1_004_001),
        ["queryid_stat_statements"]
    );
}

fn predecessor(value: Cell) -> BTreeMap<&'static str, Cell> {
    BTreeMap::from([(COLUMN, value)])
}

#[test]
fn integer_rates_subtract_exact_values_above_two_to_the_fifty_third() {
    let signed = 1_i64 << 53;
    let before = predecessor(Cell::I64(signed));
    assert_eq!(
        rate(
            Some(&Cell::I64(signed + 1)),
            Some(&before),
            COLUMN,
            Some(1_000_000),
        ),
        json!(1.0)
    );

    let unsigned = 1_u64 << 53;
    let before = predecessor(Cell::U64(unsigned));
    assert_eq!(
        rate(
            Some(&Cell::U64(unsigned + 1)),
            Some(&before),
            COLUMN,
            Some(1_000_000),
        ),
        json!(1.0)
    );
}

#[test]
fn floating_point_counters_have_their_own_delta_path() {
    let before = predecessor(Cell::F64(10.25));
    assert_eq!(
        rate(
            Some(&Cell::F64(10.75)),
            Some(&before),
            COLUMN,
            Some(500_000),
        ),
        json!(1.0)
    );
}

#[test]
fn decreasing_and_missing_counters_have_no_rate() {
    for (now, earlier) in [
        (Cell::I64(9), Cell::I64(10)),
        (Cell::U64(9), Cell::U64(10)),
        (Cell::F64(9.0), Cell::F64(10.0)),
    ] {
        let before = predecessor(earlier);
        assert_eq!(
            rate(Some(&now), Some(&before), COLUMN, Some(1_000_000)),
            Value::Null
        );
    }
    let now = Cell::I64(10);
    assert_eq!(rate(Some(&now), None, COLUMN, Some(1_000_000)), Value::Null);
    assert_eq!(
        rate(Some(&now), Some(&BTreeMap::new()), COLUMN, Some(1_000_000)),
        Value::Null
    );
}

#[test]
fn ordering_uses_the_first_candidate_present_in_the_physical_layout() {
    let fields = [
        OutputField {
            name: "total_time".to_owned(),
            column: None,
        },
        OutputField {
            name: "total_exec_time".to_owned(),
            column: Some("total_exec_time"),
        },
        OutputField {
            name: "calls".to_owned(),
            column: Some("calls"),
        },
    ];
    assert_eq!(available_field_index(&fields, "total_time"), None);
    assert_eq!(
        ["total_time", "total_exec_time", "calls"]
            .iter()
            .find_map(|name| available_field_index(&fields, name)),
        Some(1)
    );
}

#[test]
fn numeric_ordering_preserves_large_integer_precision_and_nulls() {
    let base = 1_u64 << 53;
    assert_eq!(
        compare_ordered(
            ordered_cell(&Cell::U64(base + 1)),
            ordered_cell(&Cell::U64(base)),
        ),
        Ordering::Greater
    );
    assert_eq!(
        compare_ordered(ordered_cell(&Cell::I64(-1)), ordered_cell(&Cell::I64(-2)),),
        Ordering::Greater
    );
    assert!(ordered_cell(&Cell::F64(f64::NAN)).is_none());
    assert!(ordered_cell(&Cell::F64(f64::INFINITY)).is_none());
}

#[test]
fn exact_product_comparison_orders_different_limb_lengths() {
    assert_eq!(
        compare_products(&[100, 1_000_000], &[1_048_575, 1_000_000]),
        Ordering::Less
    );
    assert_eq!(
        compare_products(&[u128::MAX, u128::MAX], &[u128::MAX, u128::MAX - 1]),
        Ordering::Greater
    );
}

fn ranked(layout_index: usize, ordinal: u64, value: Option<PageOrderValue>) -> PageRankedRow {
    ranked_for(1_002_006, layout_index, ordinal, value)
}

fn ranked_for(
    type_id: u32,
    layout_index: usize,
    ordinal: u64,
    value: Option<PageOrderValue>,
) -> PageRankedRow {
    let contract = contract(type_id).expect("fixture contract");
    PageRankedRow {
        staged: PageStagedRow {
            context_index: layout_index,
            ordinal,
            row: Row::new(contract, Vec::new()),
            identity: Vec::new(),
        },
        value,
        direction: Order::Desc,
    }
}

#[test]
fn five_thousand_statement_and_plan_candidates_keep_only_one_bounded_page() {
    const ROWS: usize = 5_000;
    const RETAINED: usize = 201;
    for type_id in [1_002_006, 1_003_001] {
        let values = (0..ROWS)
            .map(|ordinal| {
                (ordinal % 37 != 0).then_some(i128::try_from((ordinal * 7_919) % 997).unwrap())
            })
            .collect::<Vec<_>>();
        let mut expected = values
            .iter()
            .enumerate()
            .map(|(ordinal, value)| (ordinal, *value))
            .collect::<Vec<_>>();
        expected.sort_by(|(left_ordinal, left), (right_ordinal, right)| {
            right
                .cmp(left)
                .then_with(|| left_ordinal.cmp(right_ordinal))
        });
        expected.truncate(RETAINED);

        let mut page = PageRows::new(RETAINED);
        for (ordinal, value) in values.into_iter().enumerate() {
            page.push(ranked_for(
                type_id,
                0,
                u64::try_from(ordinal).unwrap(),
                value.map(PageOrderValue::Integer),
            ));
            assert!(page.retained_len() <= RETAINED);
        }
        let actual = page
            .finish()
            .into_iter()
            .map(|row| usize::try_from(row.staged.ordinal).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|(ordinal, _value)| ordinal)
                .collect::<Vec<_>>()
        );
        assert_eq!(actual.len(), RETAINED);
    }
}

#[test]
fn page_heap_is_bounded_and_ties_use_layout_then_ordinal() {
    let mut page = PageRows::new(3);
    for (layout, ordinal, value) in [(1, 1, 9), (0, 2, 9), (0, 1, 9), (0, 0, 8), (1, 0, 10)] {
        page.push(ranked(
            layout,
            ordinal,
            Some(PageOrderValue::Integer(value)),
        ));
        assert!(page.retained_len() <= 3);
    }
    let rows = page
        .finish()
        .into_iter()
        .map(|row| (row.staged.context_index, row.staged.ordinal))
        .collect::<Vec<_>>();
    assert_eq!(rows, [(1, 0), (0, 1), (0, 2)]);
}

#[test]
fn integer_rate_ordering_cross_multiplies_elapsed_time_exactly() {
    let faster = ranked(
        0,
        0,
        Some(PageOrderValue::IntegerRate {
            delta: (1_i128 << 100) + 1,
            elapsed: 2,
        }),
    );
    let slower = ranked(
        0,
        1,
        Some(PageOrderValue::IntegerRate {
            delta: 1_i128 << 99,
            elapsed: 1,
        }),
    );
    assert_eq!(faster.cmp(&slower), Ordering::Greater);
}

#[test]
fn integer_ratio_ordering_is_exact_without_cross_product_overflow() {
    let larger = PageOrderValue::IntegerRatio {
        numerator: u128::MAX - 1,
        denominator: u128::MAX - 2,
    };
    let smaller = PageOrderValue::IntegerRatio {
        numerator: u128::MAX,
        denominator: u128::MAX - 1,
    };
    assert_eq!(
        super::compare_page_order_values(Some(&larger), Some(&smaller), Order::Desc),
        Ordering::Greater
    );

    let ratio_winner = PageOrderValue::IntegerRatio {
        numerator: 60,
        denominator: 2,
    };
    let raw_winner = PageOrderValue::IntegerRatio {
        numerator: 100,
        denominator: 10,
    };
    assert_eq!(
        super::compare_page_order_values(Some(&ratio_winner), Some(&raw_winner), Order::Desc),
        Ordering::Greater
    );

    for left_numerator in 0..30 {
        for left_denominator in 1..30 {
            for right_numerator in 0..30 {
                for right_denominator in 1..30 {
                    assert_eq!(
                        super::compare_u128_ratios(
                            left_numerator,
                            left_denominator,
                            right_numerator,
                            right_denominator,
                        ),
                        (left_numerator * right_denominator)
                            .cmp(&(right_numerator * left_denominator))
                    );
                }
            }
        }
    }
}

#[test]
fn ascending_order_reverses_values_but_keeps_null_last() {
    let one = PageOrderValue::Integer(1);
    let two = PageOrderValue::Integer(2);
    assert_eq!(
        super::compare_page_order_values(Some(&one), Some(&two), Order::Asc),
        Ordering::Greater
    );
    assert_eq!(
        super::compare_page_order_values(Some(&one), None, Order::Asc),
        Ordering::Greater
    );
    assert_eq!(
        super::compare_page_order_values(None, Some(&one), Order::Asc),
        Ordering::Less
    );
}

#[test]
fn relation_search_fields_are_public_and_do_not_expose_oids() {
    assert_eq!(
        super::search_fields("pg_stat_user_tables")
            .iter()
            .map(|field| field.key)
            .collect::<Vec<_>>(),
        [
            "text",
            "database",
            "schema",
            "table_name",
            "tablespace",
            "size",
            "table_count",
            "buffer_hit",
            "seq_scan_rate",
            "change_rate",
            "autovacuum_rate",
            "autovacuum_mean",
            "xid_age"
        ]
    );
    assert_eq!(
        super::search_fields("pg_stat_user_indexes")
            .iter()
            .map(|field| field.key)
            .collect::<Vec<_>>(),
        [
            "text",
            "database",
            "schema",
            "table_name",
            "index_name",
            "access_method",
            "definition",
            "tablespace",
            "size",
            "index_count",
            "buffer_hit",
            "scan_rate"
        ]
    );
}

#[test]
fn activity_search_fields_include_pid_and_state() {
    let fields = super::search_fields("pg_stat_activity");
    assert!(fields.iter().any(|field| field.key == "pid"));
    assert!(fields.iter().any(|field| field.key == "state"));
}

#[test]
fn locks_search_fields_include_lock_mode() {
    assert!(
        super::search_fields("pg_locks")
            .iter()
            .any(|field| field.key == "lock_mode")
    );
}

#[test]
fn vacuum_search_fields_include_phase() {
    assert!(
        super::search_fields("pg_stat_progress_vacuum")
            .iter()
            .any(|field| field.key == "phase")
    );
}

#[test]
fn database_search_fields_include_deadlocks() {
    assert!(
        super::search_fields("pg_stat_database")
            .iter()
            .any(|field| field.key == "deadlocks")
    );
}

#[test]
fn cursor_round_trips_and_rejects_malformed_values() {
    let cursor = SnapshotCursor {
        segment_id: -4,
        active_position: 8,
        context_index: 11,
        ordinal: 99,
        binding: u64::MAX,
    };
    assert_eq!(
        SnapshotCursor::parse(&cursor.encode()).expect("cursor"),
        cursor
    );
    for invalid in ["", "1,2,3,4", "1,2,3,4,5,6", "x,2,3,4,5"] {
        assert!(SnapshotCursor::parse(invalid).is_err());
    }
}

#[test]
fn contributing_moments_keep_all_sources_at_the_anchor_pair() {
    let mut moments = ContributingMoments::default();
    record_contributing_moment(&mut moments, 200, 9);
    record_contributing_moment(&mut moments, 100, 9);
    moments.pinned = true;
    record_contributing_moment(&mut moments, 250, 8);
    record_contributing_moment(&mut moments, 200, 8);
    record_contributing_moment(&mut moments, 150, 7);
    record_contributing_moment(&mut moments, 150, 6);

    let current = moments.current.expect("anchor current moment");
    assert_eq!(current.at, 200);
    assert_eq!(current.segment_ids, HashSet::from([8, 9]));
    let previous = moments.previous.expect("immediately preceding moment");
    assert_eq!(previous.at, 150);
    assert_eq!(previous.segment_ids, HashSet::from([6, 7]));
}

#[test]
fn dynamic_timed_context_indices_are_unique_beyond_three_sources() {
    let source_count = 6;
    let indices = (0..2)
        .flat_map(|layout_index| {
            (0..source_count).map(move |source_index| {
                timed_context_index(layout_index, source_index, source_count)
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(indices, (0..12).collect::<Vec<_>>());
}

fn request() -> SnapshotRequest {
    SnapshotRequest {
        segment_id: 7,
        at: 11,
        sections: vec!["pg_stat_statements".to_owned()],
        fields: vec!["queryid".to_owned(), "query".to_owned()],
        by: vec!["calls".to_owned()],
        direction: Order::Desc,
        group: None,
        page_size: Some(200),
        cursor: None,
        search: Some("needle*".to_owned()),
        first_match: false,
        text: Some(80),
        filters: vec![Filter {
            column: "dbid".to_owned(),
            value: "4".to_owned(),
        }],
        type_id: Some(1_002_006),
        row_ordinal: None,
        scope: StatementScope::All,
    }
}

fn request_binding(request: &SnapshotRequest) -> u64 {
    let search = request.search.as_deref().and_then(|raw| {
        let [logical_name] = request.sections.as_slice() else {
            return None;
        };
        StructuredSearch::parse(raw, logical_name).ok()
    });
    snapshot_binding(request, search.as_ref())
}

#[test]
fn cursor_binding_covers_query_shape_but_excludes_page_size_and_cursor() {
    let baseline = request();
    let expected = request_binding(&baseline);
    let mut harmless = baseline.clone();
    harmless.page_size = Some(5_000);
    harmless.cursor = Some("opaque".to_owned());
    assert_eq!(request_binding(&harmless), expected);

    let mut variants = Vec::new();
    let mut changed = baseline.clone();
    changed.segment_id += 1;
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.at += 1;
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.sections.push("other".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.fields.reverse();
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.by.push("rows".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.direction = Order::Asc;
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.group = Some(RelationGroup::Schema);
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.search = Some("second".to_owned());
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.text = Some(81);
    variants.push(changed);
    let mut changed = baseline.clone();
    changed.filters[0].value = "5".to_owned();
    variants.push(changed);
    let mut changed = baseline;
    changed.type_id = None;
    variants.push(changed);

    for changed in variants {
        assert_ne!(request_binding(&changed), expected);
    }
}

#[test]
fn cursor_binding_uses_server_canonical_search() {
    let mut alias = request();
    alias.search = Some("db:app and text:orders".to_owned());
    let mut canonical = alias.clone();
    canonical.search = Some("database:app AND text:orders".to_owned());
    assert_eq!(request_binding(&alias), request_binding(&canonical));

    let mut spaced = request();
    spaced.sections = vec!["pg_stat_user_tables".to_owned()];
    spaced.group = Some(RelationGroup::Object);
    spaced.search = Some(" size > 100.000MB ".to_owned());
    let mut compact = spaced.clone();
    compact.search = Some("size>100MB".to_owned());
    assert_eq!(request_binding(&spaced), request_binding(&compact));
}

#[test]
fn grouped_requests_reject_mixed_phase_or_before_execution() {
    let mut grouped = request();
    grouped.sections = vec!["pg_stat_user_tables".to_owned()];
    grouped.group = Some(RelationGroup::Schema);
    grouped.search = Some("schema:public OR size>100MB".to_owned());
    assert!(prepared_search(&grouped, false).is_err());

    grouped.search = Some("(schema:public OR schema:audit) AND size>100MB".to_owned());
    assert!(prepared_search(&grouped, false).is_ok());
    grouped.search = Some("schema:public AND (size>100MB OR buffer_hit<90%)".to_owned());
    assert!(prepared_search(&grouped, false).is_ok());

    grouped.group = Some(RelationGroup::Object);
    grouped.search = Some("schema:public OR size>100MB".to_owned());
    assert!(prepared_search(&grouped, false).is_ok());
}

#[test]
fn glob_supports_substrings_wildcards_literals_and_unicode_case() {
    for (pattern, candidate, matches) in [
        ("needle", "a NEEDLE here", true),
        ("a*c", "xxa/bbcyy", true),
        ("a?c", "xxaécyy", true),
        ("a?c", "xxaéécyy", false),
        ("select (x)+[y]", "SELECT (x)+[y]", true),
        ("*Σ?", "prefix σx", true),
        ("wanted", "unrelated", false),
    ] {
        assert_eq!(GlobPattern::new(pattern).matches(candidate), matches);
    }
}

#[test]
fn literal_substring_pattern_does_not_expand_wildcards() {
    let pattern = GlobPattern::contains("A*?");
    assert!(pattern.matches("prefix a*? suffix"));
    assert!(!pattern.matches("prefix ab suffix"));
    assert!(!pattern.matches("prefix axy suffix"));
}

#[test]
fn structured_search_validates_aliases_types_escaping_and_surface_fields() {
    let parsed = StructuredSearch::parse(
        r#"query_id:-912345 and db:"Sales \"East\"""#,
        "pg_stat_statements",
    )
    .expect("valid structured search");
    assert_eq!(parsed.clauses.len(), 2);
    assert_eq!(parsed.clauses[0].key, "query_id");
    assert!(matches!(
        &parsed.clauses[0].value,
        SearchValue::Identifier(value) if value == "-912345"
    ));
    assert_eq!(parsed.clauses[1].key, "database");
    assert!(matches!(
        &parsed.clauses[1].value,
        SearchValue::Pattern(pattern) if pattern.matches("Sales \"East\"")
    ));

    for invalid in [
        "taname:orders",
        "query_id:*",
        "query_id:01",
        "query_id:9223372036854775808",
        r#"database:"unterminated"#,
        r#"database:"bad\n""#,
    ] {
        assert!(
            StructuredSearch::parse(invalid, "pg_stat_statements").is_err(),
            "{invalid}"
        );
    }
    assert!(StructuredSearch::parse("plan_id:42", "pg_stat_statements").is_err());
    assert!(StructuredSearch::parse("planid:42", "pg_store_plans").is_err());
    assert!(StructuredSearch::parse(r#"database:"""#, "pg_stat_statements").is_err());
    assert!(StructuredSearch::parse("query_id:-0", "pg_stat_statements").is_err());
    assert!(StructuredSearch::parse("select orders*", "pg_stat_statements").is_ok());
    assert!(StructuredSearch::parse("select AND orders", "pg_stat_statements").is_err());

    let process = StructuredSearch::parse(
        "username:postgres AND euser:postgres-worker AND uid:26 AND euid:27",
        "os_process",
    )
    .expect("process aliases");
    assert_eq!(
        process
            .clauses
            .iter()
            .map(|clause| clause.key)
            .collect::<Vec<_>>(),
        ["user", "effective_user", "user_id", "effective_user_id"]
    );
    for (input, canonical) in [
        ("resident_memory>2MiB", "rss>2MiB"),
        ("virtual_memory>1GiB", "vsz>1GiB"),
        ("majflt_rate>1/s", "major_fault_rate>1/s"),
        ("rchar_rate>1MiB/s", "logical_read_rate>1MiB/s"),
        ("blkdelay>50ms/s", "block_io_delay>50ms/s"),
    ] {
        assert_eq!(
            StructuredSearch::parse(input, "os_process")
                .expect("documented process alias")
                .canonical(),
            canonical
        );
    }
    for raw in ["utime>1", "rchar>1MiB", "read_bytes>1MiB", "cpu>0.1"] {
        assert!(StructuredSearch::parse(raw, "os_process").is_err(), "{raw}");
    }
}

#[test]
fn quantitative_search_registry_is_surface_wide_and_physical_names_stay_private() {
    let statements = super::search_fields("pg_stat_statements")
        .iter()
        .map(|field| field.key)
        .collect::<Vec<_>>();
    let plans = super::search_fields("pg_store_plans")
        .iter()
        .map(|field| field.key)
        .collect::<Vec<_>>();
    for shared in [
        "call_rate",
        "exec_time_rate",
        "mean_exec",
        "row_rate",
        "rows_per_call",
        "planning_time_rate",
        "planning_share",
        "shared_buffer_read_rate",
        "local_buffer_write_rate",
        "temp_buffer_read_rate",
        "shared_read_time_rate",
        "buffer_hit",
        "buffer_per_call",
        "exec_cv",
        "min_exec_since_reset",
        "max_exec_since_reset",
        "mean_exec_since_reset",
        "stddev_exec_since_reset",
    ] {
        assert!(statements.contains(&shared), "Statements: {shared}");
        assert!(plans.contains(&shared), "Plans: {shared}");
    }
    for statement_only in ["plan_rate", "wal_rate", "wal_per_call"] {
        assert!(statements.contains(&statement_only));
        assert!(!plans.contains(&statement_only));
    }
    assert!(!statements.contains(&"calls"));
    assert!(plans.contains(&"calls"));
    assert!(plans.contains(&"slow_call_rate"));
    for physical in [
        "total_exec_time",
        "shared_blks_read",
        "wal_bytes",
        "rmem_kb",
        "utime",
        "read_bytes",
    ] {
        assert!(!statements.contains(&physical));
        assert!(!plans.contains(&physical));
        assert!(
            !super::search_fields("os_process")
                .iter()
                .any(|field| field.key == physical)
        );
    }
}

#[test]
fn structured_search_limits_clause_and_value_counts() {
    let clauses = std::iter::repeat_n("role:reader", super::SEARCH_MAX_CLAUSES + 1)
        .collect::<Vec<_>>()
        .join(" AND ");
    assert!(StructuredSearch::parse(&clauses, "pg_stat_statements").is_err());
    let value = "x".repeat(super::SEARCH_MAX_VALUE_CHARS + 1);
    assert!(StructuredSearch::parse(&format!("database:{value}"), "pg_stat_statements").is_err());
}

#[test]
fn structured_search_parses_strict_exact_quantities_and_canonicalizes_them() {
    let parsed = StructuredSearch::parse(
        " schema:public and size > 100.000MB AND seq_scan_rate<0.5/s ",
        "pg_stat_user_tables",
    )
    .expect("valid comparisons");
    assert_eq!(
        parsed.canonical(),
        "schema:public AND size>100MB AND seq_scan_rate<0.5/s"
    );
    assert_eq!(parsed.clauses[0].columns, ["schemaname"]);
    assert!(matches!(parsed.expr, super::Expr::And(..)));
    assert!(matches!(
        &parsed.clauses[1].value,
        SearchValue::Quantity(quantity)
            if quantity.numerator == 100_000_000 && quantity.denominator == 1
    ));
    assert!(matches!(
        &parsed.clauses[2].value,
        SearchValue::Quantity(quantity)
            if quantity.numerator == 1 && quantity.denominator == 2
    ));

    for (expression, numerator, denominator) in [
        ("size>100MB", 100_000_000, 1),
        ("size>100MiB", 104_857_600, 1),
        ("size>0.5KiB", 512, 1),
        ("autovacuum_mean<250000us", 250, 1),
        ("buffer_hit>99.95%", 1_999, 20),
    ] {
        let parsed = StructuredSearch::parse(expression, "pg_stat_user_tables")
            .expect("valid exact quantity");
        assert!(matches!(
            &parsed.clauses[0].value,
            SearchValue::Quantity(quantity)
                if quantity.numerator == numerator && quantity.denominator == denominator
        ));
    }
}

#[test]
fn structured_search_parses_process_and_postgres_quantity_units_exactly() {
    for (surface, expression, numerator, denominator) in [
        ("os_process", "cpu_cores>0.1", 1, 10),
        ("os_process", "rss>2MiB", 2_097_152, 1),
        ("os_process", "disk_read_rate>1.5MiB/s", 1_572_864, 1),
        ("os_process", "run_delay<250us/s", 1, 4),
        ("pg_stat_statements", "exec_time_rate>0.5s/s", 500, 1),
        ("pg_stat_statements", "wal_per_call>0.5KiB", 512, 1),
        ("pg_store_plans", "rows_per_call<0.125", 1, 8),
    ] {
        let parsed = StructuredSearch::parse(expression, surface).expect("valid exact quantity");
        assert!(matches!(
            &parsed.clauses[0].value,
            SearchValue::Quantity(quantity)
                if quantity.numerator == numerator && quantity.denominator == denominator
        ));
    }
    for (surface, expression) in [
        ("os_process", "cpu_cores>1core"),
        ("os_process", "rss>2MiB/s"),
        ("os_process", "disk_read_rate>1MiB"),
        ("pg_stat_statements", "exec_time_rate>1ms"),
        ("pg_stat_statements", "wal_per_call>1MiB/s"),
        ("pg_store_plans", "rows_per_call>1/s"),
    ] {
        assert!(
            StructuredSearch::parse(expression, surface).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn structured_search_rejects_atomic_operators_units_and_not_with_spans() {
    for (expression, code, token) in [
        ("size>=100MB", "unsupported_operator", ">="),
        ("size<=100MB", "unsupported_operator", "<="),
        ("size==100MB", "unsupported_operator", "=="),
        ("size!=100MB", "unsupported_operator", "!="),
        ("size=100MB", "unsupported_operator", "="),
        ("size=>100MB", "malformed_operator", "=>"),
        ("size<>100MB", "malformed_operator", "<>"),
        ("size:100MB", "operator_not_allowed", ":"),
        ("schema>public", "operator_not_allowed", ">"),
        ("NOT size>100MB", "unsupported_syntax", "NOT"),
        ("NOT latency", "unsupported_syntax", "NOT"),
        ("size>100 MB", "whitespace_before_unit", "MB"),
    ] {
        let error = StructuredSearch::parse(expression, "pg_stat_user_tables")
            .expect_err("invalid comparison");
        assert_eq!(error.code, code, "{expression}");
        assert_eq!(
            expression.get(error.start..error.end),
            Some(token),
            "{expression}"
        );
    }
    for expression in [
        "size>0.1B",
        "size>100",
        "size>100mb",
        r#"size>"100MB""#,
        "buffer_hit>100.1%",
        "table_count>1.5",
        "size>-1MB",
        "size>1e3MB",
        "size>1,000MB",
        "size>1_MB",
        "size>NaN",
        "size>Infinity",
    ] {
        assert!(
            StructuredSearch::parse(expression, "pg_stat_user_tables").is_err(),
            "{expression}"
        );
    }
    assert!(
        StructuredSearch::parse(r#"text:"size>100MB OR (later)""#, "pg_stat_user_tables").is_ok()
    );
}

#[test]
fn structured_search_parses_boolean_precedence_groups_and_phase_rules() {
    let parsed = StructuredSearch::parse(
        "((schema:public OR schema:audit)) AND (size > 100.000MB OR buffer_hit<90%)",
        "pg_stat_user_tables",
    )
    .expect("valid boolean expression");
    assert_eq!(
        parsed.canonical(),
        "(schema:public OR schema:audit) AND (size>100MB OR buffer_hit<90%)"
    );
    assert!(matches!(parsed.expr, super::Expr::And(..)));
    parsed
        .validate_grouped_phase()
        .expect("AND may cross the grouped phase boundary");
    assert!(parsed.matches_member(|clause| {
        matches!(&clause.value, SearchValue::Pattern(pattern) if pattern.matches("audit"))
    }));
    assert!(!parsed.matches_member(|clause| {
        matches!(&clause.value, SearchValue::Pattern(pattern) if pattern.matches("private"))
    }));

    let precedence = StructuredSearch::parse(
        "schema:public OR schema:audit AND table_name:orders",
        "pg_stat_user_tables",
    )
    .expect("valid precedence");
    assert!(matches!(precedence.expr, super::Expr::Or { .. }));

    for expression in [
        "schema:public OR size>100MB",
        "(schema:public AND size>100MB) OR schema:audit",
    ] {
        let error = StructuredSearch::parse(expression, "pg_stat_user_tables")
            .expect("syntactically valid")
            .validate_grouped_phase()
            .expect_err("mixed grouped OR");
        assert_eq!(error.code, "mixed_phase_or", "{expression}");
        assert_eq!(expression.get(error.start..error.end), Some("OR"));
    }
}

#[test]
fn structured_search_boolean_diagnostics_and_bounds_have_exact_spans() {
    for (expression, code, token) in [
        ("()", "empty_group", "()"),
        ("(schema:public", "unbalanced_parenthesis", "("),
        ("schema:public)", "unbalanced_parenthesis", ")"),
        ("schema:public AND", "missing_operand", "AND"),
        ("schema:public OR OR schema:audit", "missing_operand", "OR"),
        ("AND schema:public", "missing_operand", "AND"),
        (
            "schema:public table_name:orders",
            "expected_boolean_operator",
            "table_name:orders",
        ),
    ] {
        let error = StructuredSearch::parse(expression, "pg_stat_user_tables")
            .expect_err("invalid boolean expression");
        assert_eq!(error.code, code, "{expression}");
        assert_eq!(expression.get(error.start..error.end), Some(token));
    }

    let deep = format!("{}schema:public{}", "(".repeat(5), ")".repeat(5));
    assert_eq!(
        StructuredSearch::parse(&deep, "pg_stat_user_tables")
            .expect_err("excessive group nesting")
            .code,
        "group_too_deep"
    );
    let clauses = std::iter::repeat_n("(schema:public)", super::SEARCH_MAX_CLAUSES)
        .collect::<Vec<_>>()
        .join(" OR ");
    assert!(StructuredSearch::parse(&clauses, "pg_stat_user_tables").is_ok());
    assert_eq!(
        StructuredSearch::parse(&format!("({clauses})"), "pg_stat_user_tables")
            .expect_err("excessive boolean tokens")
            .code,
        "too_many_tokens"
    );
}

#[test]
fn lifetime_cpu_time_sums_the_raw_counters_and_withholds_a_missing_half() {
    let contract = contract(1_100_001).expect("os_process contract");
    let at = |name: &str| {
        contract
            .columns
            .iter()
            .position(|column| column.name == name)
            .expect("projected column")
    };
    let row = |user: Option<i64>, system: Option<i64>| {
        let mut cells = vec![Cell::Null; contract.columns.len()];
        if let Some(value) = user {
            cells[at("utime")] = Cell::I64(value);
        }
        if let Some(value) = system {
            cells[at("stime")] = Cell::I64(value);
        }
        Row::new(contract, cells)
    };
    assert_eq!(
        scheduled_ticks(&row(Some(38_341), Some(7_788))),
        json!("46129")
    );
    assert_eq!(scheduled_ticks(&row(Some(0), Some(0))), json!("0"));
    assert_eq!(scheduled_ticks(&row(Some(1), None)), Value::Null);
    assert_eq!(scheduled_ticks(&row(None, Some(1))), Value::Null);
    assert_eq!(scheduled_ticks(&row(Some(i64::MAX), Some(1))), Value::Null);
}

#[test]
fn a_workload_scope_hides_collector_statements_and_counts_them() {
    const MONITOR: &str = "/* kronika:1.0.0 pg_sources.rs */ select monitor";
    let payload = fixture_payload(|interner, buffers| {
        let application = StrId(
            interner
                .intern(b"select 42")
                .expect("intern application text")
                .get(),
        );
        let collector = StrId(
            interner
                .intern(MONITOR.as_bytes())
                .expect("intern collector text")
                .get(),
        );
        for dbid in 1..=3 {
            buffers
                .push(statement(dbid, Some(application)))
                .expect("application row fits");
        }
        for dbid in 4..=5 {
            buffers
                .push(statement(dbid, Some(collector)))
                .expect("collector row fits");
        }
        buffers.push(statement(6, None)).expect("textless row fits");
    });
    let mut request = snapshot_request("pg_stat_statements", &["dbid", "query"]);
    request.page_size = Some(10);
    request.scope = StatementScope::Workload;
    let records = snapshot_records(&payload, request.clone());

    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        4,
        "three application rows and the textless row stay"
    );
    assert!(rows.iter().all(|row| row["values"][1] != json!(MONITOR)));
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("page trailer");
    assert_eq!(page["eligible"], json!("4"));
    assert_eq!(page["excluded"], json!("2"));

    request.scope = StatementScope::All;
    let records = snapshot_records(&payload, request.clone());
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("page trailer");
    assert_eq!(page["eligible"], json!("6"));
    assert_eq!(page["excluded"], json!("0"));

    // A cursor minted under one scope is refused under the other.
    let all = snapshot_binding(&request, None);
    request.scope = StatementScope::Workload;
    assert_ne!(all, snapshot_binding(&request, None));
}
