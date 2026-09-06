//! Query-output parity across native and in-memory finished-segment sources.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use base64 as _;
use icu_collator as _;
use icu_locale_core as _;
use kronika_format::DictLimits;
use kronika_index::build_from_reader;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_query::snapshot::{
    CurrentSnapshotQuery, FinderQuery, FinderResult, FinderSurface, PlainRowOut, ProcessRowOut,
    RelationRow, SnapshotPoint, execute_current_plain, execute_current_relation, execute_plain,
    execute_processes, execute_relation,
};
use kronika_query::{
    CatalogRequest, EventsQuery, EventsRepresentation, EventsResult, Filter, FinishedDataset,
    HeatmapBatchQuery, HeatmapBatchResult, HeatmapItemQuery, HeatmapView, HourPart, HourRequest,
    HourSeriesRequest, IndexProvider, IndexRequest, MemoryIndexProvider, NormalizedRanking,
    QueryContext, QueryDataset, QueryRequest, QuerySink, RelationGroup, RelationKind,
    RowDetailResult, StatementScope, TimeRange, Window, detail_locator, execute, execute_events,
    execute_heatmap_batch, execute_row_detail, validate_row_detail_ref,
};
use kronika_reader::Reader;
use kronika_registry::instance_metadata::InstanceMetadata;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_process::OsProcess;
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_log::{PgLogErrors, PgLogTempFiles};
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_database::PgStatDatabaseV1;
use kronika_registry::pg_stat_progress_vacuum::PgStatProgressVacuumV1;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::pg_stat_user_indexes::PgStatUserIndexesV1;
use kronika_registry::pg_stat_user_tables::PgStatUserTablesV1;
use kronika_registry::pg_store_plans::PgStorePlansOsscV1;
use kronika_registry::{StrId, Ts};
use kronika_store::{EmbeddedSource, PosixSource};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde as _;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SEGMENT_ID_TEXT: &str = "1709164800000000";
const ZMS: &[u8] = include_bytes!("../../kronika-format/tests/fixtures/minimal.zms");
const HEATMAP_FROM: i64 = SEGMENT_ID;
const HEATMAP_TO: i64 = HEATMAP_FROM + 1_000_000;

#[derive(Default)]
struct Records(Vec<u8>);

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0.extend_from_slice(&bytes);
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn catalog_bytes(dataset: Arc<dyn QueryDataset>) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0, false);
    let execution = execute(&context, QueryRequest::Catalog(CatalogRequest::default()))
        .expect("prepare catalog query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream catalog query");
    records.0
}

fn heatmap_query(top: usize) -> HeatmapBatchQuery {
    HeatmapBatchQuery {
        range: TimeRange::new(HEATMAP_FROM, HEATMAP_TO + 1).expect("heatmap range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_cpu".to_owned(),
                fields: vec!["user".to_owned()],
                top,
            },
            view: HeatmapView::Grid {
                columns: 1,
                group: Vec::new(),
                type_id: None,
            },
            scope: StatementScope::All,
        }],
    }
}

fn heatmap_bytes(dataset: Arc<dyn QueryDataset>) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0, false);
    let query =
        kronika_query::validate_heatmap_request(heatmap_query(1)).expect("validate heatmap query");
    let execution = execute(&context, QueryRequest::Heatmap(query)).expect("prepare heatmap query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream heatmap query");
    records.0
}

fn hour_bytes(dataset: Arc<dyn QueryDataset>, request: HourRequest) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0b11, false);
    let execution = execute(&context, QueryRequest::Hour(request)).expect("prepare hour query");
    let mut records = Records::default();
    execution.stream(&mut records).expect("stream hour query");
    records.0
}

fn indexed_bytes(
    dataset: Arc<dyn QueryDataset>,
    indexes: Arc<dyn IndexProvider>,
    request: QueryRequest,
) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0b11, false).with_index_provider(indexes);
    let execution = execute(&context, request).expect("prepare indexed query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream indexed query");
    records.0
}

fn series_hour_request(
    window: Window,
    section: &str,
    fields: Vec<String>,
    filters: Vec<Filter>,
    group: Option<RelationGroup>,
) -> HourRequest {
    HourRequest {
        window,
        series: Some(HourSeriesRequest {
            section: section.to_owned(),
            fields,
            filters,
            type_id: None,
            group,
            scope: StatementScope::All,
        }),
        part: HourPart::Combined,
        segments: None,
        active: None,
    }
}

fn ndjson(bytes: &[u8]) -> Vec<serde_json::Value> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice(record).expect("NDJSON record"))
        .collect()
}

fn ranking_only_query() -> HeatmapBatchQuery {
    let top_one = HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_cpu".to_owned(),
            fields: vec!["user".to_owned()],
            top: 1,
        },
        view: HeatmapView::RankingOnly,
        scope: StatementScope::All,
    };
    let mut top_two = top_one.clone();
    top_two.ranking.top = 2;
    HeatmapBatchQuery {
        range: TimeRange::new(HEATMAP_FROM, HEATMAP_TO + 1).expect("ranking range"),
        items: vec![top_one.clone(), top_two, top_one],
    }
}

fn ranking_only_result(
    dataset: Arc<dyn QueryDataset>,
    query: HeatmapBatchQuery,
) -> HeatmapBatchResult {
    let context = QueryContext::new(dataset, 0, false);
    execute_heatmap_batch(&context, query, &Records::default())
        .expect("execute typed ranking batch")
}

fn events_query() -> EventsQuery {
    EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 100).expect("events range"),
        Some(vec![
            "pg_log_temp_files".to_owned(),
            "pg_log_errors".to_owned(),
        ]),
        EventsRepresentation::Occurrences,
        3,
    )
    .expect("events query")
}

fn events_result(dataset: Arc<dyn QueryDataset>, query: EventsQuery) -> EventsResult {
    let context = QueryContext::new(dataset, 0, false);
    execute_events(&context, query, &Records::default()).expect("execute typed events query")
}

fn row_detail_result(dataset: Arc<dyn QueryDataset>, detail_ref: &str) -> RowDetailResult {
    let context = QueryContext::new(dataset, 0, false);
    let request = validate_row_detail_ref(detail_ref).expect("validate row detail reference");
    execute_row_detail(&context, request, &Records::default()).expect("execute typed row detail")
}

#[derive(Debug)]
struct CountingRowDetailDataset {
    inner: FinishedDataset<PosixSource>,
    opens: AtomicUsize,
}

impl QueryDataset for CountingRowDetailDataset {
    fn catalog(
        &self,
    ) -> Result<Box<dyn kronika_query::CapturedCatalog + '_>, kronika_query::QueryError> {
        self.inner.catalog()
    }

    fn segment(&self, id: i64) -> Result<kronika_query::DatasetListing, kronika_query::QueryError> {
        self.inner.segment(id)
    }

    fn open(
        &self,
        segment: &kronika_query::DatasetSegment,
    ) -> Result<kronika_reader::Segment, kronika_query::QueryError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.inner.open(segment)
    }

    fn at_active_position(
        &self,
        segment: &kronika_query::DatasetSegment,
        position: u64,
    ) -> Result<kronika_query::DatasetSegment, kronika_query::QueryError> {
        self.inner.at_active_position(segment, position)
    }
}

fn finder_query(surface: FinderSurface) -> FinderQuery {
    FinderQuery {
        surface,
        point: SnapshotPoint::LatestRecorded,
        search: None,
        order: None,
        group: matches!(surface, FinderSurface::Tables | FinderSurface::Indexes)
            .then_some(RelationGroup::Object),
        limit: 4,
    }
}

fn process_result_json(result: FinderResult<ProcessRowOut>) -> serde_json::Value {
    let rows = result
        .rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "pid": row.pid,
                "ppid": row.ppid,
                "segment_id": row.segment_id,
                "type_id": row.type_id,
                "row_ordinal": row.row_ordinal,
                "at": row.at,
                "identity": row.identity,
                "fields": row.fields,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "rows": rows,
        "truncated": result.truncated,
        "as_of": result.as_of,
    })
}

fn plain_result_json(result: FinderResult<PlainRowOut>) -> serde_json::Value {
    let rows = result
        .rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "segment_id": row.segment_id,
                "type_id": row.type_id,
                "row_ordinal": row.row_ordinal,
                "at": row.at,
                "identity": row.identity,
                "fields": row.fields,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "rows": rows,
        "truncated": result.truncated,
        "as_of": result.as_of,
    })
}

fn relation_result_json(
    result: FinderResult<RelationRow>,
    kind: RelationKind,
) -> serde_json::Value {
    let rows = result
        .rows
        .into_iter()
        .map(|row| {
            let metrics = row
                .metrics
                .into_iter()
                .map(|(name, metric)| (name, metric.map_or(serde_json::Value::Null, |m| m.json())))
                .collect::<serde_json::Map<_, _>>();
            serde_json::json!({
                "key": row.key.json(kind, RelationGroup::Object),
                "metrics": metrics,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "rows": rows,
        "truncated": result.truncated,
        "as_of": result.as_of,
    })
}

fn finder_result_json(context: &QueryContext, surface: FinderSurface) -> serde_json::Value {
    let query = finder_query(surface);
    match surface {
        FinderSurface::Processes => process_result_json(
            execute_processes(context, &query, &|| false).expect("execute process finder"),
        ),
        FinderSurface::Tables | FinderSurface::Indexes => {
            let kind = if surface == FinderSurface::Tables {
                RelationKind::Tables
            } else {
                RelationKind::Indexes
            };
            relation_result_json(
                execute_relation(context, &query, &|| false).expect("execute relation finder"),
                kind,
            )
        }
        _ => plain_result_json(
            execute_plain(context, &query, &|| false).expect("execute plain finder"),
        ),
    }
}

fn write_posix_fixture(root: &Path, segment_id: SegmentId) {
    let address = SegmentAddress::new(segment_id).expect("segment address");
    let day = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    std::fs::create_dir_all(&day).expect("create fixture day");
    std::fs::write(day.join(address.zms_name()), ZMS).expect("write finished ZMS fixture");
}

fn finished_path(root: &Path, segment_id: SegmentId) -> std::path::PathBuf {
    let address = SegmentAddress::new(segment_id).expect("segment address");
    root.join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name())
}

const fn parity_process(timestamp: i64, label: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(timestamp),
        pid: 41,
        starttime: Ts(SEGMENT_ID - 1_000_000),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'R',
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
        vmem_kb: 1_024,
        rmem_kb: 512,
        vswap_kb: 0,
        syscr: Some(0),
        syscw: Some(0),
        rchar: Some(0),
        wchar: Some(0),
        read_bytes: Some(0),
        write_bytes: Some(0),
        cancelled_write_bytes: Some(0),
        exit_signal: 17,
        scope: 0,
    }
}

const fn parity_table(
    timestamp: i64,
    database: StrId,
    schema: StrId,
    table: StrId,
) -> PgStatUserTablesV1 {
    PgStatUserTablesV1 {
        ts: Ts(timestamp),
        datid: 1,
        datname: database,
        relid: 11,
        schemaname: schema,
        relname: table,
        tablespace_oid: None,
        tablespace: None,
        seq_scan: 10,
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

const fn parity_index(
    timestamp: i64,
    database: StrId,
    schema: StrId,
    table: StrId,
    label: StrId,
    scans: i64,
) -> PgStatUserIndexesV1 {
    PgStatUserIndexesV1 {
        ts: Ts(timestamp),
        datid: 1,
        datname: database,
        indexrelid: 12,
        relid: 11,
        schemaname: schema,
        relname: table,
        indexrelname: label,
        tablespace_oid: 1_663,
        tablespace: Some(label),
        idx_scan: scans,
        idx_tup_read: 0,
        idx_tup_fetch: 0,
        main_fork_bytes: 8_192,
        indisunique: true,
        indisprimary: true,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname: label,
        indexdef: Some(label),
        idx_blks_read: 0,
        idx_blks_hit: 0,
    }
}

const fn parity_database(timestamp: i64, database: StrId, commits: i64) -> PgStatDatabaseV1 {
    PgStatDatabaseV1 {
        ts: Ts(timestamp),
        datid: 1,
        datname: Some(database),
        numbackends: Some(3),
        xact_commit: commits,
        xact_rollback: 0,
        blks_read: 0,
        blks_hit: 0,
        tup_returned: 0,
        tup_fetched: 0,
        tup_inserted: 0,
        tup_updated: 0,
        tup_deleted: 0,
        conflicts: 0,
        temp_files: 0,
        temp_bytes: 0,
        deadlocks: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        stats_reset: None,
        frozen_xid_age: Some(7),
        min_mxid_age: Some(5),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture counters are the exactly representable values 10 and 20"
)]
fn parity_statement(timestamp: i64, label: StrId, calls: i64) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(timestamp),
        queryid: Some(71),
        userid: 72,
        dbid: 1,
        datname: Some(label),
        usename: Some(label),
        query: Some(label),
        calls,
        rows: calls * 2,
        plans: calls,
        total_exec_time: calls as f64 * 4.0,
        total_plan_time: calls as f64,
        min_exec_time: 1.0,
        max_exec_time: 4.0,
        mean_exec_time: 2.0,
        stddev_exec_time: 0.5,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.5,
        stddev_plan_time: 0.1,
        shared_blks_hit: calls,
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

#[expect(
    clippy::cast_precision_loss,
    reason = "fixture counters are the exactly representable values 10 and 20"
)]
fn parity_plan(timestamp: i64, label: StrId, calls: i64) -> PgStorePlansOsscV1 {
    PgStorePlansOsscV1 {
        ts: Ts(timestamp),
        queryid: 71,
        planid: -7,
        userid: 72,
        dbid: 1,
        datname: Some(label),
        usename: Some(label),
        plan: Some(label),
        calls,
        total_time: calls as f64 * 3.0,
        min_time: 1.0,
        max_time: 3.0,
        mean_time: 2.0,
        stddev_time: 0.5,
        rows: calls * 2,
        shared_blks_hit: calls,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        shared_blk_read_time: 0.0,
        shared_blk_write_time: 0.0,
        local_blk_read_time: 0.0,
        local_blk_write_time: 0.0,
        temp_blk_read_time: 0.0,
        temp_blk_write_time: 0.0,
        first_call: Ts(timestamp - 1),
        last_call: Ts(timestamp),
    }
}

fn fixture_label(interner: &mut Interner, value: &[u8]) -> StrId {
    StrId(interner.intern(value).expect("intern fixture label").get())
}

#[expect(
    clippy::too_many_lines,
    reason = "one shared segment fixture covers every query family without duplicate writers"
)]
fn write_heatmap_fixture(root: &Path, segment_id: SegmentId) -> Arc<[u8]> {
    let data_root = DataRoot::open(root).expect("open heatmap data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire heatmap writer");
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open heatmap journal");
    let mut interner = Interner::new(DictLimits::default());
    let label = fixture_label(&mut interner, b"parity");
    let database = fixture_label(&mut interner, b"parity_database");
    let schema = fixture_label(&mut interner, b"parity_schema");
    let table = fixture_label(&mut interner, b"parity_table");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(HEATMAP_FROM),
            hostname: label,
            kernel_version: label,
            environment: 0,
            clock_ticks_per_sec: 100,
            page_size_bytes: 4_096,
            boot_id: label,
            btime: Ts(1),
            postgresql_enabled: false,
            postgresql_interval_seconds: 30,
            postgresql_effective_cpus: None,
        })
        .expect("metadata row fits");
    for (timestamp, aggregate, first, second) in [(HEATMAP_FROM, 0, 0, 0), (HEATMAP_TO, 30, 10, 20)]
    {
        for (cpu_id, user) in [(-1, aggregate), (0, first), (1, second)] {
            buffers
                .push(OsCpu {
                    ts: Ts(timestamp),
                    cpu_id,
                    user,
                    nice: 0,
                    system: 0,
                    idle: 0,
                    iowait: 0,
                    irq: 0,
                    softirq: 0,
                    steal: 0,
                    guest: 0,
                    guest_nice: 0,
                    scope: 0,
                })
                .expect("CPU row fits");
        }
    }
    let process = parity_process(HEATMAP_FROM, label);
    buffers.push(process).expect("base process row fits");
    buffers
        .push(OsProcess {
            ts: Ts(HEATMAP_TO),
            utime: 100,
            ..process
        })
        .expect("current process row fits");
    let relation = parity_table(HEATMAP_FROM, database, schema, table);
    buffers.push(relation).expect("base relation row fits");
    buffers
        .push(PgStatUserTablesV1 {
            ts: Ts(HEATMAP_TO),
            seq_scan: 20,
            ..relation
        })
        .expect("current relation row fits");
    for (timestamp, scans) in [(HEATMAP_FROM, 3), (HEATMAP_TO, 9)] {
        buffers
            .push(parity_index(
                timestamp, database, schema, table, label, scans,
            ))
            .expect("index row fits");
    }
    buffers
        .push(PgStatActivityV3 {
            ts: Ts(HEATMAP_TO),
            pid: 42,
            leader_pid: None,
            datid: Some(1),
            datname: Some(database),
            usename: Some(label),
            application_name: label,
            client_addr: label,
            backend_type: label,
            state: Some(label),
            wait_event_type: None,
            wait_event: None,
            query: Some(label),
            query_id: Some(71),
            backend_xid_age: Some(7),
            backend_xmin_age: Some(5),
            backend_start: Ts(HEATMAP_FROM),
            xact_start: Some(Ts(HEATMAP_FROM)),
            query_start: Some(Ts(HEATMAP_FROM)),
            state_change: Some(Ts(HEATMAP_FROM)),
        })
        .expect("activity row fits");
    buffers
        .push(PgLocksV2 {
            ts: Ts(HEATMAP_TO),
            pid: 42,
            blocked_by: vec![7],
            datid: 1,
            datname: database,
            usename: Some(label),
            application_name: label,
            client_addr: label,
            backend_type: label,
            state: Some(label),
            wait_event_type: Some(label),
            wait_event: Some(label),
            query: label,
            backend_xid_age: Some(7),
            backend_xmin_age: Some(5),
            backend_start: Some(Ts(HEATMAP_FROM)),
            xact_start: Some(Ts(HEATMAP_FROM)),
            query_start: Some(Ts(HEATMAP_FROM)),
            state_change: Some(Ts(HEATMAP_FROM)),
            lock_locktype: Some(label),
            lock_mode: Some(label),
            lock_database: Some(1),
            lock_relation: Some(11),
            lock_relname: Some(table),
            lock_page: None,
            lock_tuple: None,
            lock_virtualxid: None,
            lock_transactionid: None,
            lock_classid: None,
            lock_objid: None,
            lock_objsubid: None,
            lock_target: Some(label),
            waitstart: Some(Ts(HEATMAP_FROM)),
        })
        .expect("lock row fits");
    buffers
        .push(PgStatProgressVacuumV1 {
            ts: Ts(HEATMAP_TO),
            pid: 43,
            datid: 1,
            datname: database,
            relid: 11,
            schemaname: Some(schema),
            relname: Some(table),
            is_autovacuum: true,
            phase: label,
            heap_blks_total: 100,
            heap_blks_scanned: 40,
            heap_blks_vacuumed: 20,
            index_vacuum_count: 1,
            max_dead_tuples: 1_000,
            num_dead_tuples: 250,
        })
        .expect("vacuum row fits");
    for (timestamp, calls) in [(HEATMAP_FROM, 10), (HEATMAP_TO, 20)] {
        buffers
            .push(parity_database(timestamp, database, calls))
            .expect("database row fits");
        buffers
            .push(parity_statement(timestamp, label, calls))
            .expect("statement row fits");
        buffers
            .push(parity_plan(timestamp, label, calls))
            .expect("plan row fits");
    }
    let dictionary = dict::encode(interner.window()).expect("encode parity dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode heatmap rows")
        .expect("nonempty heatmap rows");
    journal
        .append(segment_id, &part)
        .expect("append heatmap rows");
    let summary = write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("segment address"),
    )
    .expect("write heatmap segment");
    journal.reset().expect("reset heatmap journal");
    drop(journal);
    drop(owner);

    let payload: Arc<[u8]> = std::fs::read(finished_path(root, segment_id))
        .expect("read heatmap segment")
        .into();
    assert_eq!(
        u64::try_from(payload.len()).expect("payload length fits u64"),
        summary.bytes,
        "writer byte count must match the published segment"
    );
    payload
}

fn write_events_fixture(root: &Path, segment_id: SegmentId) -> Arc<[u8]> {
    let data_root = DataRoot::open(root).expect("open events data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire events writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open events journal");
    let mut interner = Interner::new(DictLimits::default());
    let source_file = StrId(
        interner
            .intern(b"postgresql.log")
            .expect("intern source file")
            .get(),
    );
    let mut buffers = SectionBuffers::new();
    for (timestamp, pattern) in [
        (SEGMENT_ID + 10, b"error-a".as_slice()),
        (SEGMENT_ID + 20, b"error-b".as_slice()),
    ] {
        let pattern = StrId(interner.intern(pattern).expect("intern pattern").get());
        buffers
            .push(PgLogErrors {
                ts: Ts(timestamp),
                system_identifier: Some(42),
                source_file,
                severity: 0,
                category: 8,
                sqlstate: None,
                pattern,
                count: 1,
                sample: pattern,
                detail: None,
                hint: None,
                context: None,
                statement: None,
                database: None,
                username: None,
            })
            .expect("error row fits");
    }
    for (timestamp, size_bytes) in [(SEGMENT_ID + 10, 100), (SEGMENT_ID + 30, 300)] {
        buffers
            .push(PgLogTempFiles {
                ts: Ts(timestamp),
                system_identifier: Some(42),
                source_file,
                path: None,
                size_bytes,
                statement: None,
            })
            .expect("temporary-file row fits");
    }
    let dictionary = dict::encode(interner.window()).expect("encode events dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode event rows")
        .expect("nonempty event rows");
    journal
        .append(segment_id, &part)
        .expect("append event rows");
    let summary = write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("segment address"),
    )
    .expect("write events segment");
    journal.reset().expect("reset events journal");
    drop(journal);
    drop(owner);

    let payload: Arc<[u8]> = std::fs::read(finished_path(root, segment_id))
        .expect("read events segment")
        .into();
    assert_eq!(
        u64::try_from(payload.len()).expect("payload length fits u64"),
        summary.bytes,
        "writer byte count must match the published segment"
    );
    payload
}

fn write_process_segment(root: &Path, segment_id: SegmentId, timestamp: i64, pid: i32, utime: i64) {
    let data_root = DataRoot::open(root).expect("open process data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire process writer");
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open process journal");
    let mut interner = Interner::new(DictLimits::default());
    let label = fixture_label(&mut interner, b"row-detail");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(OsProcess {
            ts: Ts(timestamp),
            pid,
            utime,
            ..parity_process(timestamp, label)
        })
        .expect("process row fits");
    let dictionary = dict::encode(interner.window()).expect("encode process dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode process rows")
        .expect("nonempty process rows");
    journal
        .append(segment_id, &part)
        .expect("append process rows");
    write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("process segment address"),
    )
    .expect("write process segment");
    journal.reset().expect("reset process journal");
}

#[test]
fn catalog_query_is_byte_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    write_posix_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_bytes = catalog_bytes(Arc::new(FinishedDataset::new(posix)));

    let payload: Arc<[u8]> = Arc::from(ZMS);
    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    )
    .expect("embedded source");
    let embedded_bytes = catalog_bytes(Arc::new(FinishedDataset::new(embedded)));

    assert_eq!(posix_bytes, embedded_bytes);
    let records = embedded_bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice::<serde_json::Value>(record).expect("NDJSON record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1]["record"], "finished_segment");
    assert_eq!(
        records[1]["id"].as_str(),
        Some(SEGMENT_ID_TEXT),
        "embedded execution must keep the caller-supplied segment identity"
    );
}

#[test]
fn heatmap_query_is_byte_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(posix));
    let posix_bytes = heatmap_bytes(Arc::clone(&posix_dataset));

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(embedded));
    let embedded_bytes = heatmap_bytes(Arc::clone(&embedded_dataset));

    assert_eq!(posix_bytes, embedded_bytes);
    let records = embedded_bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice::<serde_json::Value>(record).expect("NDJSON record"))
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|record| record["record"].as_str().expect("record kind"))
            .collect::<Vec<_>>(),
        ["heatmap", "heatmap_row", "heatmap_band", "heatmap_band"]
    );
    assert_eq!(records[0]["from"], HEATMAP_FROM.to_string());
    assert_eq!(records[0]["to"], HEATMAP_TO.to_string());
    assert_eq!(records[0]["entity_count"], 2);
    assert_eq!(records[0]["top"], 1);
    assert_eq!(records[0]["others_count"], 1);
    assert_eq!(records[1]["identity"], serde_json::json!([1]));
    assert_eq!(records[1]["total"], 20.0);
    assert_eq!(records[2]["total"], 30.0);
    assert_eq!(records[3]["total"], 10.0);

    let posix_error = kronika_query::validate_heatmap_request(heatmap_query(0))
        .expect_err("POSIX request rejects zero top");
    let embedded_error = kronika_query::validate_heatmap_request(heatmap_query(0))
        .expect_err("embedded request rejects zero top");
    assert_eq!(posix_error.code(), embedded_error.code());
    assert_eq!(posix_error.parameter(), embedded_error.parameter());
    assert_eq!(posix_error.to_string(), embedded_error.to_string());
}

#[test]
fn hour_products_are_byte_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);
    let window = Window {
        from: Some(HEATMAP_FROM),
        to: Some(HEATMAP_TO),
    };
    let requests = [
        series_hour_request(window, "os_cpu", vec!["user".to_owned()], Vec::new(), None),
        HourRequest {
            window,
            series: None,
            part: HourPart::Lanes,
            segments: Some(vec![SEGMENT_ID]),
            active: None,
        },
        series_hour_request(
            window,
            "os_process_summary",
            vec!["user_cores".to_owned()],
            Vec::new(),
            None,
        ),
        series_hour_request(window, "postgresql_summary", Vec::new(), Vec::new(), None),
        series_hour_request(
            window,
            "pg_stat_user_tables",
            vec!["seq_scan".to_owned()],
            vec![Filter {
                column: "datid".to_owned(),
                value: "1".to_owned(),
            }],
            Some(RelationGroup::Database),
        ),
    ];

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(posix));

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(embedded));

    let mut outputs = Vec::new();
    for request in requests {
        let posix_bytes = hour_bytes(Arc::clone(&posix_dataset), request.clone());
        let embedded_bytes = hour_bytes(Arc::clone(&embedded_dataset), request);
        assert_eq!(posix_bytes, embedded_bytes);
        outputs.push(embedded_bytes);
    }

    assert!(
        outputs[0]
            .windows(b"\"record\":\"row\"".len())
            .any(|bytes| { bytes == b"\"record\":\"row\"" })
    );
    assert!(
        outputs[1]
            .windows(b"\"lane\":\"cpu_busy\"".len())
            .any(|bytes| { bytes == b"\"lane\":\"cpu_busy\"" })
    );
    let end = HEATMAP_TO.to_string();
    let process = ndjson(&outputs[2]);
    assert!(process.iter().any(|record| {
        record["record"] == "row"
            && record["timestamp"].as_str() == Some(end.as_str())
            && record["values"][0].as_f64() == Some(1.0)
    }));
    let postgresql = ndjson(&outputs[3]);
    assert!(postgresql.iter().any(|record| {
        record["record"] == "row"
            && record["values"][0] == 4
            && record["values"][9].as_f64() == Some(100.0)
    }));
    let relation = ndjson(&outputs[4]);
    assert!(relation.iter().any(|record| {
        record["record"] == "relation"
            && record["sample_to"].as_str() == Some(end.as_str())
            && record["values"]["seq_scan"].as_f64() == Some(10.0)
    }));
}

#[test]
fn native_built_index_drives_real_posix_and_owned_embedded_queries() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    drop(write_heatmap_fixture(directory.path(), segment_id));

    let reader = Reader::open(directory.path()).expect("native reader");
    let listing = reader.segments(..).expect("native segment listing");
    let segment_ref = listing.segments.first().expect("finished segment");
    let segment = reader.open_segment(segment_ref).expect("native segment");
    let encoded_index = build_from_reader(&reader, segment_ref, &segment)
        .expect("native canonical index build")
        .encode()
        .expect("canonical index encoding");
    let provider: Arc<dyn IndexProvider> = Arc::new(
        MemoryIndexProvider::new(segment_id, encoded_index).expect("memory index provider"),
    );

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(posix));
    let payload = std::fs::read(finished_path(directory.path(), segment_id))
        .expect("read finished ZMS into owned bytes");
    let payload_len = payload.len();
    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload,
        u64::try_from(payload_len).expect("payload length fits u64"),
    )
    .expect("owned embedded source");
    let embedded_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(embedded));

    let requests = [
        QueryRequest::Index(IndexRequest {
            segment_id: SEGMENT_ID,
            section: "health".to_owned(),
        }),
        QueryRequest::Hour(HourRequest {
            window: Window {
                from: Some(HEATMAP_FROM),
                to: Some(HEATMAP_TO),
            },
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        }),
    ];
    for request in requests {
        let posix = indexed_bytes(
            Arc::clone(&posix_dataset),
            Arc::clone(&provider),
            request.clone(),
        );
        let embedded = indexed_bytes(
            Arc::clone(&embedded_dataset),
            Arc::clone(&provider),
            request,
        );
        assert_eq!(posix, embedded);
        assert!(
            posix
                .windows(b"\"record\":\"index\"".len())
                .any(|bytes| bytes == b"\"record\":\"index\"")
        );
    }
}

#[test]
fn ranking_only_batch_is_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);
    let query = ranking_only_query();

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_result = ranking_only_result(Arc::new(FinishedDataset::new(posix)), query.clone());

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_result = ranking_only_result(Arc::new(FinishedDataset::new(embedded)), query);

    assert_eq!(posix_result, embedded_result);
    let results = &embedded_result.results;
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0], results[2],
        "exact duplicate items stay in place"
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.ranking.top)
            .collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert!(results.iter().all(|result| result.grid.is_none()));
    assert_eq!(results[0].entity_count, 2);
    assert_eq!(results[0].totals_total, Some(30.0));
    assert_eq!(results[0].others_total, Some(10.0));
    assert_eq!(results[0].entities.len(), 1);
    assert_eq!(
        results[0].entities[0].identity["cpu_id"],
        serde_json::json!(1)
    );
    assert_eq!(results[0].entities[0].total, Some(20.0));
    assert_eq!(results[1].entity_count, 2);
    assert_eq!(results[1].others_total, None);
    assert_eq!(results[1].entities.len(), 2);
    assert_eq!(
        results[1].entities[0].identity["cpu_id"],
        serde_json::json!(1)
    );
    assert_eq!(results[1].entities[0].total, Some(20.0));
    assert_eq!(
        results[1].entities[1].identity["cpu_id"],
        serde_json::json!(0)
    );
    assert_eq!(results[1].entities[1].total, Some(10.0));
}

#[test]
fn events_result_is_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_events_fixture(directory.path(), segment_id);
    let query = events_query();

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_result = events_result(Arc::new(FinishedDataset::new(posix)), query.clone());

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_result = events_result(Arc::new(FinishedDataset::new(embedded)), query);

    assert_eq!(posix_result, embedded_result);
    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = &embedded_result
    else {
        panic!("occurrence result");
    };
    assert!(*truncated);
    assert_eq!(occurrences.len(), 3);
    let detail_refs = occurrences
        .iter()
        .map(|occurrence| occurrence.detail_ref().expect("valid detail reference"))
        .collect::<Vec<_>>();
    assert!(detail_refs.iter().all(|detail_ref| !detail_ref.is_empty()));
    assert_eq!(detail_refs, {
        let EventsResult::Occurrences { occurrences, .. } = &posix_result else {
            panic!("occurrence result");
        };
        occurrences
            .iter()
            .map(|occurrence| occurrence.detail_ref().expect("valid detail reference"))
            .collect::<Vec<_>>()
    });
    let wire = serde_json::to_value(&embedded_result).expect("serialize typed event result");
    assert_eq!(
        wire["occurrences"]
            .as_array()
            .expect("occurrence array")
            .iter()
            .map(|occurrence| (
                occurrence["source"].as_str().expect("event source"),
                occurrence["detail_locator"]["at"]
                    .as_str()
                    .expect("event timestamp"),
            ))
            .collect::<Vec<_>>(),
        [
            ("pg_log_temp_files", "1709164800000010"),
            ("pg_log_errors", "1709164800000010"),
            ("pg_log_errors", "1709164800000020"),
        ]
    );
}

#[test]
fn row_detail_is_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_events_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(posix));
    let listing = events_result(Arc::clone(&posix_dataset), events_query());
    let EventsResult::Occurrences { occurrences, .. } = listing else {
        panic!("occurrence result");
    };
    let detail_ref = occurrences
        .get(1)
        .expect("error occurrence after temporary-file tie")
        .detail_ref()
        .expect("error detail reference");
    let posix_result = row_detail_result(posix_dataset, &detail_ref);

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_result = row_detail_result(Arc::new(FinishedDataset::new(embedded)), &detail_ref);

    assert_eq!(posix_result, embedded_result);
    assert_eq!(embedded_result.section, "pg_log_errors");
    assert_eq!(embedded_result.at, SEGMENT_ID + 10);
    assert_eq!(embedded_result.fields["at"], (SEGMENT_ID + 10).to_string());
    assert_eq!(embedded_result.fields["severity"], 0);
    assert_eq!(embedded_result.fields["severity_label"], "error");
    assert_eq!(embedded_result.fields["category"], 8);
    assert_eq!(embedded_result.fields["category_label"], "auth");
    assert_eq!(embedded_result.fields["pattern"], "error-a");
    assert_eq!(
        embedded_result.fields["sample"],
        serde_json::json!({
            "full_len": "7",
            "sha256": null,
            "stored_text": "error-a",
            "truncated": false,
        })
    );
}

#[test]
fn row_detail_uses_the_target_identity_predecessor_once_per_needed_segment() {
    let root = tempfile::tempdir().expect("process fixture directory");
    let first = SegmentId::new(SEGMENT_ID).expect("first segment id");
    let unrelated = SegmentId::new(SEGMENT_ID + 1_000_000).expect("unrelated segment id");
    let anchor = SegmentId::new(SEGMENT_ID + 2_000_000).expect("anchor segment id");
    let older = SegmentId::new(SEGMENT_ID - 1_000_000).expect("older segment id");
    write_process_segment(root.path(), older, SEGMENT_ID - 999_990, 41, 1);
    write_process_segment(root.path(), first, SEGMENT_ID + 10, 41, 10);
    write_process_segment(root.path(), unrelated, SEGMENT_ID + 1_000_010, 99, 100);
    write_process_segment(root.path(), anchor, SEGMENT_ID + 2_000_010, 41, 20);

    let dataset = Arc::new(CountingRowDetailDataset {
        inner: FinishedDataset::new(PosixSource::open(root.path()).expect("POSIX source")),
        opens: AtomicUsize::new(0),
    });
    let detail_ref = detail_locator(
        "os_process",
        anchor.get(),
        SEGMENT_ID + 2_000_010,
        1_100_001,
        0,
        std::iter::once(("pid".to_owned(), serde_json::Value::String("41".to_owned()))).collect(),
    )
    .detail_ref()
    .expect("process detail reference");
    let dataset_for_query: Arc<dyn QueryDataset> = Arc::<CountingRowDetailDataset>::clone(&dataset);
    let result = row_detail_result(dataset_for_query, &detail_ref);

    assert_eq!(result.fields["utime"], serde_json::json!(5.0));
    assert_eq!(
        dataset.opens.load(Ordering::Relaxed),
        3,
        "prepare opens the anchor once, then only the unrelated and matching predecessor segments"
    );
}

#[test]
fn all_snapshot_finders_are_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    let posix_context = QueryContext::new(Arc::new(FinishedDataset::new(posix)), 0b11, false);

    let embedded = EmbeddedSource::from_owned(
        segment_id,
        payload.as_ref().to_vec(),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    let embedded_context = QueryContext::new(Arc::new(FinishedDataset::new(embedded)), 0b11, false);

    for surface in [
        FinderSurface::Processes,
        FinderSurface::Tables,
        FinderSurface::Indexes,
        FinderSurface::Activity,
        FinderSurface::Locks,
        FinderSurface::Vacuum,
        FinderSurface::Databases,
        FinderSurface::Statements,
        FinderSurface::Plans,
    ] {
        let posix_result = finder_result_json(&posix_context, surface);
        let embedded_result = finder_result_json(&embedded_context, surface);
        assert_eq!(posix_result, embedded_result, "{surface:?} backend parity");
        assert_eq!(embedded_result["truncated"], false, "{surface:?}");
        assert_eq!(embedded_result["as_of"], HEATMAP_TO, "{surface:?}");
        assert_eq!(
            embedded_result["rows"].as_array().map(Vec::len),
            Some(1),
            "{surface:?} must return its representative row"
        );

        let (pointer, expected) = match surface {
            FinderSurface::Processes => ("/rows/0/fields/comm", serde_json::json!("parity")),
            FinderSurface::Tables => ("/rows/0/metrics/seq_scan", serde_json::json!(10.0)),
            FinderSurface::Indexes => ("/rows/0/metrics/idx_scan", serde_json::json!(6.0)),
            FinderSurface::Activity => ("/rows/0/fields/pid", serde_json::json!(42)),
            FinderSurface::Locks => ("/rows/0/fields/blocked_by", serde_json::json!([7])),
            FinderSurface::Vacuum => ("/rows/0/fields/heap_blks_scanned", serde_json::json!("40")),
            FinderSurface::Databases => ("/rows/0/fields/xact_commit", serde_json::json!(10.0)),
            FinderSurface::Statements => ("/rows/0/fields/calls", serde_json::json!(10.0)),
            FinderSurface::Plans => ("/rows/0/fields/calls", serde_json::json!("20")),
        };
        assert_eq!(
            embedded_result.pointer(pointer),
            Some(&expected),
            "{surface:?} substantive value"
        );
    }

    let plain_query = CurrentSnapshotQuery {
        logical_name: "instance_metadata".to_owned(),
        fields: vec!["hostname".to_owned()],
        order: None,
        group: None,
        limit: 2,
    };
    let posix_plain = execute_current_plain(&posix_context, plain_query.clone(), &|| false)
        .expect("execute POSIX current plain")
        .map(plain_result_json);
    let embedded_plain = execute_current_plain(&embedded_context, plain_query, &|| false)
        .expect("execute embedded current plain")
        .map(plain_result_json);
    assert_eq!(posix_plain, embedded_plain);
    let plain = embedded_plain.expect("current instance metadata");
    assert_eq!(plain["rows"][0]["fields"]["hostname"], "parity");

    let relation_query = CurrentSnapshotQuery {
        logical_name: "pg_stat_user_tables".to_owned(),
        fields: vec!["seq_scan".to_owned()],
        order: None,
        group: Some(RelationGroup::Object),
        limit: 2,
    };
    let posix_relation =
        execute_current_relation(&posix_context, relation_query.clone(), &|| false)
            .expect("execute POSIX current relation")
            .map(|result| relation_result_json(result, RelationKind::Tables));
    let embedded_relation = execute_current_relation(&embedded_context, relation_query, &|| false)
        .expect("execute embedded current relation")
        .map(|result| relation_result_json(result, RelationKind::Tables));
    assert_eq!(posix_relation, embedded_relation);
    let relation = embedded_relation.expect("current table relation");
    assert_eq!(relation["rows"][0]["key"]["relname"], "parity_table");
    assert_eq!(relation["rows"][0]["metrics"]["seq_scan"], 10.0);
}

#[path = "finished_source_parity/statement_scope.rs"]
mod statement_scope;
