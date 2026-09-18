//! Typed Heatmap semantics and bounded-resource regressions.

use std::cell::Cell;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use base64 as _;
use icu_collator as _;
use icu_locale_core as _;
use kronika_format::{DictLimits, ReadAt};
use kronika_index as _;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::{Segment, SegmentKind};
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{StrId, Ts};
use kronika_store::{
    EmbeddedResource, EmbeddedSource, ImmutableSegmentSource, PosixSource, ResourceCatalog,
    ResourceError, ResourceListing, SegmentResource, SharedSegmentBytes,
};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde as _;
use serde_json as _;

use kronika_query::{
    CapturedCatalog, DatasetListing, DatasetSegment, FinishedDataset, HeatmapBatchQuery,
    HeatmapItemQuery, HeatmapView, NormalizedRanking, OpaqueCapture, QueryContext, QueryDataset,
    QueryError, QuerySink, SegmentSelection, StatementScope, TimeRange, execute_heatmap_batch,
};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const FIRST_TS: i64 = SEGMENT_ID;
const LAST_TS: i64 = FIRST_TS + 1_000_000;

#[derive(Debug, Default)]
struct ResourceCounts {
    opens: AtomicUsize,
    current: AtomicUsize,
    peak: AtomicUsize,
    read_calls: AtomicUsize,
    read_bytes: AtomicUsize,
}

impl ResourceCounts {
    fn opened(&self) {
        self.opens.fetch_add(1, Ordering::Relaxed);
        let current = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(current, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.opens.load(Ordering::Relaxed),
            self.current.load(Ordering::Relaxed),
            self.peak.load(Ordering::Relaxed),
            self.read_calls.load(Ordering::Relaxed),
            self.read_bytes.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug, Clone)]
struct TrackingSource {
    inner: EmbeddedSource,
    counts: Arc<ResourceCounts>,
}

#[derive(Debug)]
struct TrackingBytes {
    inner: SharedSegmentBytes,
    counts: Arc<ResourceCounts>,
}

impl Drop for TrackingBytes {
    fn drop(&mut self) {
        self.counts.current.fetch_sub(1, Ordering::Relaxed);
    }
}

impl ReadAt for TrackingBytes {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.counts.read_calls.fetch_add(1, Ordering::Relaxed);
        self.counts
            .read_bytes
            .fetch_add(buf.len(), Ordering::Relaxed);
        self.inner.read_exact_at(buf, offset)
    }

    fn byte_len(&self) -> io::Result<u64> {
        self.inner.byte_len()
    }
}

impl ResourceCatalog for TrackingSource {
    type Resource = EmbeddedResource;

    fn resources(&self) -> Result<ResourceListing<Self::Resource>, ResourceError> {
        self.inner.resources()
    }
}

impl ImmutableSegmentSource for TrackingSource {
    type Bytes = TrackingBytes;

    fn open_resource(
        &self,
        resource: &SegmentResource<Self::Resource>,
    ) -> Result<Self::Bytes, ResourceError> {
        let inner = self.inner.open_resource(resource)?;
        self.counts.opened();
        Ok(TrackingBytes {
            inner,
            counts: Arc::clone(&self.counts),
        })
    }

    fn validate_opened(
        &self,
        resource: &SegmentResource<Self::Resource>,
        bytes: &Self::Bytes,
    ) -> Result<(), ResourceError> {
        self.inner.validate_opened(resource, &bytes.inner)
    }
}

#[derive(Debug)]
struct CountingDataset {
    inner: FinishedDataset<TrackingSource>,
    opens: AtomicUsize,
}

impl QueryDataset for CountingDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        self.inner.catalog()
    }

    fn segment(&self, id: i64) -> Result<DatasetListing, QueryError> {
        self.inner.segment(id)
    }

    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        self.inner.open(segment)
    }

    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        self.inner.at_active_position(segment, position)
    }
}

#[derive(Debug)]
struct VersionedDataset {
    versions: [FinishedDataset<EmbeddedSource>; 2],
    current: AtomicUsize,
    advance_after_selection: AtomicBool,
}

impl VersionedDataset {
    const fn selected(&self, version: usize) -> &FinishedDataset<EmbeddedSource> {
        &self.versions[version]
    }
}

#[derive(Debug)]
struct VersionedCatalog<'a> {
    dataset: &'a VersionedDataset,
    version: usize,
    ranges: Vec<(i64, i64)>,
}

#[derive(Debug, Clone)]
struct VersionCapture {
    version: usize,
    descriptor: DatasetSegment,
}

impl CapturedCatalog for VersionedCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        &self.ranges
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        let listing = self
            .dataset
            .selected(self.version)
            .catalog()?
            .segments(selection)?;
        if self
            .dataset
            .advance_after_selection
            .swap(false, Ordering::SeqCst)
        {
            self.dataset.current.store(1, Ordering::SeqCst);
        }
        Ok(DatasetListing {
            segments: listing
                .segments
                .into_iter()
                .map(|descriptor| {
                    DatasetSegment::new(
                        OpaqueCapture::new(VersionCapture {
                            version: self.version,
                            descriptor: descriptor.clone(),
                        }),
                        descriptor.id(),
                        SegmentKind::Active,
                        descriptor.min_ts(),
                        descriptor.max_ts(),
                        Some(1),
                        Arc::from(descriptor.sections().to_vec()),
                    )
                })
                .collect(),
            warnings: listing.warnings,
        })
    }
}

impl QueryDataset for VersionedDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        let version = self.current.load(Ordering::SeqCst);
        let ranges = self.selected(version).catalog()?.ranges().to_vec();
        Ok(Box::new(VersionedCatalog {
            dataset: self,
            version,
            ranges,
        }))
    }

    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("heatmap range execution does not select one segment")
    }

    fn open(&self, segment: &DatasetSegment) -> Result<Segment, QueryError> {
        let capture = segment
            .capture()
            .downcast_ref::<VersionCapture>()
            .expect("versioned descriptor capture");
        self.selected(capture.version).open(&capture.descriptor)
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        unreachable!("heatmap batch does not repin an already captured segment")
    }
}

#[derive(Default)]
struct NeverCancelled;

impl QuerySink for NeverCancelled {
    fn record(&mut self, _bytes: Vec<u8>) -> bool {
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

struct CancelAfterFirstPoll(Cell<bool>);

impl QuerySink for CancelAfterFirstPoll {
    fn record(&mut self, _bytes: Vec<u8>) -> bool {
        true
    }

    fn cancelled(&self) -> bool {
        self.0.replace(true)
    }
}

fn ranking(top: usize) -> HeatmapItemQuery {
    HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_cpu".to_owned(),
            fields: vec!["user".to_owned()],
            top,
        },
        view: HeatmapView::RankingOnly,
        scope: StatementScope::All,
    }
}

fn batch(items: Vec<HeatmapItemQuery>) -> HeatmapBatchQuery {
    HeatmapBatchQuery {
        range: TimeRange::new(FIRST_TS, LAST_TS + 1).expect("valid heatmap range"),
        items,
    }
}

fn context(payload: &Arc<[u8]>) -> (QueryContext, Arc<CountingDataset>, Arc<ResourceCounts>) {
    let source = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("segment id"),
        payload.as_ref().to_vec(),
        u64::MAX,
    )
    .expect("embedded segment");
    let counts = Arc::new(ResourceCounts::default());
    let dataset = Arc::new(CountingDataset {
        inner: FinishedDataset::new(TrackingSource {
            inner: source,
            counts: Arc::clone(&counts),
        }),
        opens: AtomicUsize::new(0),
    });
    let query_dataset: Arc<dyn QueryDataset> = Arc::<CountingDataset>::clone(&dataset);
    (QueryContext::new(query_dataset, 0, false), dataset, counts)
}

fn cpu_payload(first: i64, second: i64) -> Arc<[u8]> {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut buffers = SectionBuffers::new();
    for (timestamp, first_value, second_value) in [(FIRST_TS, 0, 0), (LAST_TS, first, second)] {
        for (cpu_id, user) in [
            (-1, first_value + second_value),
            (0, first_value),
            (1, second_value),
        ] {
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
    let part = buffers
        .flush(&[])
        .expect("encode CPU rows")
        .expect("nonempty CPU rows");
    let segment_id = SegmentId::new(SEGMENT_ID).expect("segment id");
    journal.append(segment_id, &part).expect("append CPU rows");
    let address = SegmentAddress::new(segment_id).expect("segment address");
    write_segment(&journal, &owner, address).expect("finish CPU segment");
    journal.reset().expect("reset fixture journal");
    drop(journal);
    drop(owner);

    let path = root
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    std::fs::read(path).expect("read CPU segment").into()
}

#[test]
fn same_section_batch_and_duplicate_use_one_physical_scan() {
    let payload = cpu_payload(10, 20);
    let (single_context, single_dataset, single_resources) = context(&payload);
    let single = execute_heatmap_batch(&single_context, batch(vec![ranking(1)]), &NeverCancelled)
        .expect("single ranking");
    let single_reads = single_resources.snapshot();

    let (batch_context, batch_dataset, batch_resources) = context(&payload);
    let result = execute_heatmap_batch(
        &batch_context,
        batch(vec![ranking(1), ranking(2), ranking(1)]),
        &NeverCancelled,
    )
    .expect("shared ranking batch");
    let batch_reads = batch_resources.snapshot();

    assert_eq!(single_dataset.opens.load(Ordering::Relaxed), 1);
    assert_eq!(batch_dataset.opens.load(Ordering::Relaxed), 1);
    assert_eq!(batch_reads.3, single_reads.3, "row scan read calls");
    assert_eq!(batch_reads.4, single_reads.4, "row scan bytes");
    assert_eq!(batch_reads.1, 0, "all opened resources were released");
    assert_eq!(batch_reads.2, 1, "at most one resource was open");
    assert_eq!(result.results.len(), 3);
    assert_eq!(result.results[0], result.results[2]);
    assert_eq!(result.results[0].coverage.window_rows, 4);
    assert_eq!(result.results[0].entity_count, 2);
    assert_eq!(result.results[0].entities.len(), 1);
    assert_eq!(result.results[0].entities[0].identity["cpu_id"], 1);
    assert_eq!(result.results[0].entities[0].total, Some(20.0));
    assert_eq!(result.results[0].others_total, Some(10.0));
    assert_eq!(result.results[1].entities.len(), 2);
    assert_eq!(result.results[1].others_total, None);
    assert_eq!(
        result
            .results
            .iter()
            .map(|item| item.ranking.top)
            .collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert_eq!(single.results[0], result.results[0]);
}

#[test]
fn typed_batch_opens_the_active_version_captured_by_selection() {
    let old = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("segment id"),
        cpu_payload(10, 20).as_ref().to_vec(),
        u64::MAX,
    )
    .expect("old segment");
    let current = EmbeddedSource::from_owned(
        SegmentId::new(SEGMENT_ID).expect("segment id"),
        cpu_payload(100, 1).as_ref().to_vec(),
        u64::MAX,
    )
    .expect("current segment");
    let dataset = Arc::new(VersionedDataset {
        versions: [FinishedDataset::new(old), FinishedDataset::new(current)],
        current: AtomicUsize::new(0),
        advance_after_selection: AtomicBool::new(true),
    });
    let context = QueryContext::new(dataset, 0, false);

    let captured = execute_heatmap_batch(&context, batch(vec![ranking(1)]), &NeverCancelled)
        .expect("captured active result");
    let current = execute_heatmap_batch(&context, batch(vec![ranking(1)]), &NeverCancelled)
        .expect("current active result");

    assert_eq!(captured.results[0].entities[0].identity["cpu_id"], 1);
    assert_eq!(captured.results[0].entities[0].total, Some(20.0));
    assert_eq!(current.results[0].entities[0].identity["cpu_id"], 0);
    assert_eq!(current.results[0].entities[0].total, Some(100.0));
}

#[test]
fn cancellation_after_open_releases_the_segment() {
    let payload = cpu_payload(10, 20);
    let (context, dataset, resources) = context(&payload);
    let error = execute_heatmap_batch(
        &context,
        batch(vec![ranking(1)]),
        &CancelAfterFirstPoll(Cell::new(false)),
    )
    .expect_err("cancellation must stop the scan");

    assert_eq!(error.ranking_index(), 0);
    assert_eq!(error.to_string(), "rankings[0]: request cancelled");
    assert_eq!(dataset.opens.load(Ordering::Relaxed), 1);
    let counts = resources.snapshot();
    assert_eq!(counts.1, 0, "the cancelled scan released its resource");
    assert_eq!(counts.2, 1, "cancellation never overlaps resources");
}

#[test]
fn validation_reports_the_expanded_index_and_ordered_options_without_opening_data() {
    let payload = cpu_payload(10, 20);
    let (context, dataset, _resources) = context(&payload);
    let missing = HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_mountinfo".to_owned(),
            fields: vec!["missing".to_owned()],
            top: 1,
        },
        view: HeatmapView::RankingOnly,
        scope: StatementScope::All,
    };
    let error = execute_heatmap_batch(&context, batch(vec![ranking(1), missing]), &NeverCancelled)
        .expect_err("the second ranking has an unknown field");

    assert_eq!(error.ranking_index(), 1);
    assert_eq!(error.to_string(), "rankings[1]: no such column \"missing\"");
    assert_eq!(
        error.valid_options(),
        [
            "total_bytes",
            "free_bytes",
            "total_inodes",
            "available_inodes"
        ]
    );
    assert_eq!(
        dataset.opens.load(Ordering::Relaxed),
        0,
        "validation fails before catalog or segment I/O"
    );
}

const APPLICATION_QUERY_ID: i64 = 1;
const COLLECTOR_QUERY_ID: i64 = 2;

const fn statement(
    ts: i64,
    queryid: i64,
    total_exec_time: f64,
    query: StrId,
) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(ts),
        queryid: Some(queryid),
        userid: 10,
        dbid: 5,
        datname: None,
        usename: None,
        query: Some(query),
        calls: 1,
        rows: 0,
        plans: 0,
        total_exec_time,
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

/// One application statement and one collector statement, the collector's far heavier.
fn statements_payload() -> Arc<[u8]> {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_root = DataRoot::open(root.path()).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let application = StrId(
        interner
            .intern(b"select 42")
            .expect("intern application text")
            .get(),
    );
    let collector = StrId(
        interner
            .intern(b"/* kronika:1.0.0 pg_sources.rs */ select monitor")
            .expect("intern collector text")
            .get(),
    );
    let mut buffers = SectionBuffers::new();
    for (timestamp, application_time, collector_time) in
        [(FIRST_TS, 0.0, 0.0), (LAST_TS, 10.0, 1_000.0)]
    {
        buffers
            .push(statement(
                timestamp,
                APPLICATION_QUERY_ID,
                application_time,
                application,
            ))
            .expect("application row fits");
        buffers
            .push(statement(
                timestamp,
                COLLECTOR_QUERY_ID,
                collector_time,
                collector,
            ))
            .expect("collector row fits");
    }
    let dictionary = dict::encode(interner.window()).expect("statement dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode statement rows")
        .expect("nonempty statement rows");
    let segment_id = SegmentId::new(SEGMENT_ID).expect("segment id");
    journal
        .append(segment_id, &part)
        .expect("append statement rows");
    let address = SegmentAddress::new(segment_id).expect("segment address");
    write_segment(&journal, &owner, address).expect("finish statement segment");
    journal.reset().expect("reset journal");
    drop(journal);
    drop(owner);

    let path = root
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    std::fs::read(path).expect("read statement segment").into()
}

fn statements_ranking(scope: StatementScope) -> HeatmapItemQuery {
    HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "pg_stat_statements".to_owned(),
            fields: vec!["total_exec_time".to_owned()],
            top: 1,
        },
        view: HeatmapView::RankingOnly,
        scope,
    }
}

#[test]
fn a_workload_scope_ranks_statements_without_the_collectors_own() {
    let payload = statements_payload();
    let (context, _dataset, _resources) = context(&payload);
    let result = execute_heatmap_batch(
        &context,
        batch(vec![
            statements_ranking(StatementScope::All),
            statements_ranking(StatementScope::Workload),
        ]),
        &NeverCancelled,
    )
    .expect("statement rankings");

    let unscoped = &result.results[0].entities;
    assert_eq!(unscoped.len(), 1);
    assert_eq!(
        unscoped[0].identity["query_id"].as_str(),
        Some(COLLECTOR_QUERY_ID.to_string().as_str())
    );
    assert_eq!(unscoped[0].total, Some(1_000.0));
    let workload = &result.results[1].entities;
    assert_eq!(workload.len(), 1);
    assert_eq!(
        workload[0].identity["query_id"].as_str(),
        Some(APPLICATION_QUERY_ID.to_string().as_str())
    );
    assert_eq!(workload[0].total, Some(10.0));
}

#[test]
fn a_workload_scope_is_refused_outside_statements() {
    let payload = cpu_payload(10, 20);
    let (context, _dataset, _resources) = context(&payload);
    let mut scoped = ranking(1);
    scoped.scope = StatementScope::Workload;
    let error = execute_heatmap_batch(&context, batch(vec![scoped]), &NeverCancelled)
        .expect_err("a CPU ranking has no statement scope");

    assert_eq!(error.ranking_index(), 0);
    assert_eq!(
        error.to_string(),
        "rankings[0]: scope=workload applies only to pg_stat_statements"
    );
}

// Edge columns: a grid reads one column past each edge of its range so the
// spans crossing the edges close the first and last columns.

const EDGE_COLUMN: i64 = 100_000_000;
const EDGE_COLUMNS: i64 = 12;
const EDGE_FROM: i64 = SEGMENT_ID + 3_600_000_000;
const EDGE_TO: i64 = EDGE_FROM + EDGE_COLUMNS * EDGE_COLUMN;

/// CPU 0 sampled `phase` past the start of every column from `first` to
/// `last` (column offsets from the range start, possibly outside it); the
/// `user` counter grows by 100 per column, one tick per second.
fn phased_cpu_rows(phase: i64, first: i64, last: i64) -> Vec<(i32, i64, i64)> {
    (first..=last)
        .map(|column| {
            (
                0,
                EDGE_FROM + column * EDGE_COLUMN + phase,
                (column + 10) * 100,
            )
        })
        .collect()
}

const fn cpu_row(cpu_id: i32, timestamp: i64, user: i64) -> OsCpu {
    OsCpu {
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
    }
}

fn write_cpu_segment(root: &std::path::Path, segment_id: i64, rows: &[(i32, i64, i64)]) -> Vec<u8> {
    write_rows(root, segment_id, |buffers| {
        for (cpu_id, timestamp, user) in rows {
            buffers
                .push(cpu_row(*cpu_id, *timestamp, *user))
                .expect("CPU row fits");
        }
    })
}

fn write_rows(
    root: &std::path::Path,
    segment_id: i64,
    fill: impl FnOnce(&mut SectionBuffers),
) -> Vec<u8> {
    let data_root = DataRoot::open(root).expect("data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut buffers = SectionBuffers::new();
    fill(&mut buffers);
    let part = buffers
        .flush(&[])
        .expect("encode rows")
        .expect("nonempty rows");
    let segment_id = SegmentId::new(segment_id).expect("segment id");
    journal.append(segment_id, &part).expect("append rows");
    let address = SegmentAddress::new(segment_id).expect("segment address");
    write_segment(&journal, &owner, address).expect("finish segment");
    journal.reset().expect("reset fixture journal");
    drop(journal);
    drop(owner);
    let path = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    std::fs::read(path).expect("read segment")
}

fn edge_payload(rows: &[(i32, i64, i64)]) -> Arc<[u8]> {
    let root = tempfile::tempdir().expect("fixture directory");
    write_cpu_segment(root.path(), SEGMENT_ID, rows).into()
}

fn cpu_grid() -> HeatmapItemQuery {
    HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_cpu".to_owned(),
            fields: vec!["user".to_owned()],
            top: 1,
        },
        view: HeatmapView::Grid {
            columns: usize::try_from(EDGE_COLUMNS).expect("column count"),
            group: Vec::new(),
            type_id: None,
        },
        scope: StatementScope::All,
    }
}

fn edge_batch() -> HeatmapBatchQuery {
    HeatmapBatchQuery {
        range: TimeRange::new(EDGE_FROM, EDGE_TO).expect("valid heatmap range"),
        items: vec![cpu_grid()],
    }
}

fn edge_grid(context: &QueryContext) -> kronika_query::HeatmapItemResult {
    let mut result =
        execute_heatmap_batch(context, edge_batch(), &NeverCancelled).expect("edge grid");
    result.results.remove(0)
}

fn cells_of(result: &kronika_query::HeatmapItemResult) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let grid = result.grid.as_ref().expect("grid view");
    let entity = result.entities.first().expect("ranked entity");
    (
        entity.cells.clone().expect("entity cells"),
        grid.totals.cells.clone(),
    )
}

#[test]
fn the_samples_beside_the_range_close_both_edge_columns() {
    let payload = edge_payload(&phased_cpu_rows(30_000_000, -1, 12));
    let (context, _dataset, _resources) = context(&payload);
    let result = edge_grid(&context);

    let (cells, totals) = cells_of(&result);
    assert_eq!(cells, vec![Some(1.0); 12]);
    assert_eq!(totals, vec![Some(1.0); 12]);
    // The ranking window still covers only the samples inside the range.
    assert_eq!(result.entities[0].total, Some(1_100.0));
    assert_eq!(result.entity_count, 1);
    assert_eq!(result.coverage.window_rows, 12);
}

#[test]
fn a_late_sampling_phase_opens_the_first_column_from_the_sample_before() {
    let with_neighbour = edge_payload(&phased_cpu_rows(70_000_000, -1, 12));
    let (closed, _dataset, _resources) = context(&with_neighbour);
    let (cells, totals) = cells_of(&edge_grid(&closed));
    assert_eq!(cells, vec![Some(1.0); 12]);
    assert_eq!(totals, vec![Some(1.0); 12]);

    let without_neighbour = edge_payload(&phased_cpu_rows(70_000_000, 0, 12));
    let (open, _dataset, _resources) = context(&without_neighbour);
    let (cells, _totals) = cells_of(&edge_grid(&open));
    assert_eq!(cells[0], None);
    assert_eq!(cells[1..], vec![Some(1.0); 11]);
}

#[test]
fn a_range_without_a_later_sample_leaves_its_last_column_open() {
    let payload = edge_payload(&phased_cpu_rows(30_000_000, -1, 11));
    let (context, _dataset, _resources) = context(&payload);
    let (cells, totals) = cells_of(&edge_grid(&context));
    assert_eq!(cells[..11], vec![Some(1.0); 11]);
    assert_eq!(cells[11], None);
    assert_eq!(totals[11], None);
}

#[test]
fn a_counter_reset_after_the_range_leaves_the_last_column_open() {
    let mut rows = phased_cpu_rows(30_000_000, -1, 12);
    rows.last_mut().expect("later sample").2 = 0;
    let payload = edge_payload(&rows);
    let (context, _dataset, _resources) = context(&payload);
    let (cells, totals) = cells_of(&edge_grid(&context));
    assert_eq!(cells[..11], vec![Some(1.0); 11]);
    assert_eq!(cells[11], None);
    assert_eq!(totals[11], None);
}

#[test]
fn an_entity_seen_only_beside_the_range_is_not_ranked() {
    let mut rows = phased_cpu_rows(30_000_000, -1, 12);
    rows.push((1, EDGE_FROM - 70_000_000, 5));
    rows.push((1, EDGE_TO + 30_000_000, 6));
    let payload = edge_payload(&rows);
    let (context, _dataset, _resources) = context(&payload);
    let result = edge_grid(&context);
    assert_eq!(result.entity_count, 1);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].identity["cpu_id"], 0);
    assert_eq!(result.coverage.window_rows, 12);
}

#[test]
fn the_neighbouring_segments_supply_the_edge_samples() {
    let root = tempfile::tempdir().expect("fixture directory");
    let rows = phased_cpu_rows(30_000_000, -2, 13);
    write_cpu_segment(root.path(), EDGE_FROM - 600_000_000, &rows[..2]);
    write_cpu_segment(root.path(), EDGE_FROM, &rows[2..14]);
    write_cpu_segment(root.path(), EDGE_TO, &rows[14..]);
    let source = PosixSource::open(root.path()).expect("posix source");
    let dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(source));
    let context = QueryContext::new(dataset, 0, false);

    let result = edge_grid(&context);
    let (cells, totals) = cells_of(&result);
    assert_eq!(cells, vec![Some(1.0); 12]);
    assert_eq!(totals, vec![Some(1.0); 12]);
    assert_eq!(result.entities[0].total, Some(1_100.0));
}

#[test]
fn a_counter_reset_beside_the_range_does_not_blank_the_edge_column() {
    let rows = [
        (0, EDGE_FROM - 30_000_000, 10_000),
        (0, EDGE_FROM + 20_000_000, 100),
        (0, EDGE_FROM + 80_000_000, 400),
        (0, EDGE_TO - 80_000_000, 1_000),
        (0, EDGE_TO - 20_000_000, 1_300),
        (0, EDGE_TO + 30_000_000, 5),
    ];
    let payload = edge_payload(&rows);
    let (context, _dataset, _resources) = context(&payload);
    let result = edge_grid(&context);
    let (cells, totals) = cells_of(&result);
    assert_eq!(cells[0], Some(5.0));
    assert_eq!(cells[11], Some(5.0));
    assert_eq!(totals[0], Some(5.0));
    assert_eq!(totals[11], Some(5.0));
    // The internal gap exceeds fifteen minutes and cannot supply a rate.
    assert_eq!(cells[6], None);
    assert_eq!(result.entities[0].total, Some(1_200.0));
}

fn loadavg_grid_payload(phase: i64, first: i64, last: i64) -> Arc<[u8]> {
    let root = tempfile::tempdir().expect("fixture directory");
    write_rows(root.path(), SEGMENT_ID, |buffers| {
        for column in first..=last {
            buffers
                .push(OsLoadavg {
                    ts: Ts(EDGE_FROM + column * EDGE_COLUMN + phase),
                    load1: 0.0,
                    load5: 0.0,
                    load15: 0.0,
                    running: i32::try_from(column + 20).expect("small count"),
                    total: 100,
                    scope: 0,
                })
                .expect("loadavg row fits");
        }
    })
    .into()
}

fn loadavg_grid(context: &QueryContext) -> kronika_query::HeatmapItemResult {
    let query = HeatmapBatchQuery {
        range: TimeRange::new(EDGE_FROM, EDGE_TO).expect("valid heatmap range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_loadavg".to_owned(),
                fields: vec!["running".to_owned()],
                top: 1,
            },
            view: HeatmapView::Grid {
                columns: usize::try_from(EDGE_COLUMNS).expect("column count"),
                group: Vec::new(),
                type_id: None,
            },
            scope: StatementScope::All,
        }],
    };
    let mut result = execute_heatmap_batch(context, query, &NeverCancelled).expect("gauge grid");
    result.results.remove(0)
}

#[test]
fn gauges_keep_their_last_sample_inside_the_range() {
    let late = loadavg_grid_payload(70_000_000, -1, 12);
    let (late_context, _dataset, _resources) = context(&late);
    let (cells, totals) = cells_of(&loadavg_grid(&late_context));
    let expected: Vec<Option<f64>> = (0..12).map(|column| Some(f64::from(column + 20))).collect();
    assert_eq!(cells, expected);
    assert_eq!(totals, expected);

    let early = loadavg_grid_payload(30_000_000, -1, 12);
    let (early_context, _dataset, _resources) = context(&early);
    let (cells, _totals) = cells_of(&loadavg_grid(&early_context));
    // Unchanged midpoint placement: the first column keeps its last sample,
    // the last column stays open even though a later sample exists.
    assert_eq!(cells[0], Some(21.0));
    assert_eq!(cells[10], Some(31.0));
    assert_eq!(cells[11], None);
}

fn posix_context(root: &std::path::Path) -> QueryContext {
    let source = PosixSource::open(root).expect("posix source");
    let dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(source));
    QueryContext::new(dataset, 0, false)
}

#[test]
fn edge_sample_admission_is_independent_of_segment_packaging() {
    for (offset, expected) in [(600_000_000, 49_000.0 / 680.0), (900_000_001, 5.0)] {
        let rows = [
            (0, EDGE_FROM + 20_000_000, 100),
            (0, EDGE_FROM + 80_000_000, 400),
            (0, EDGE_TO - 80_000_000, 1_000),
            (0, EDGE_TO - 20_000_000, 1_300),
            (0, EDGE_TO + offset, 50_000),
        ];
        let combined = tempfile::tempdir().expect("combined directory");
        write_cpu_segment(combined.path(), EDGE_FROM, &rows);
        let combined = edge_grid(&posix_context(combined.path()));
        let split = tempfile::tempdir().expect("split directory");
        write_cpu_segment(split.path(), EDGE_FROM, &rows[..4]);
        write_cpu_segment(split.path(), EDGE_TO + offset, &rows[4..]);
        let split = edge_grid(&posix_context(split.path()));
        assert_eq!(cells_of(&combined), cells_of(&split), "offset {offset}");
        assert_eq!(cells_of(&combined).0[11], Some(expected));
        assert_eq!(combined.entities[0].total, Some(1_200.0));
    }
}

#[test]
fn overlapping_segments_keep_the_nearest_preceding_sample() {
    let first = [
        (0, EDGE_FROM - 90_000_000, 100),
        (0, EDGE_FROM + 20_000_000, 200),
        (0, EDGE_FROM + 80_000_000, 500),
    ];
    let second = [(0, EDGE_FROM - 10_000_000, 150)];
    let overlapping = tempfile::tempdir().expect("overlapping directory");
    write_cpu_segment(overlapping.path(), EDGE_FROM - 100_000_000, &first);
    write_cpu_segment(overlapping.path(), EDGE_FROM - 10_000_000, &second);
    let overlapping = edge_grid(&posix_context(overlapping.path()));
    let combined = edge_payload(&[first[0], second[0], first[1], first[2]]);
    let (context, _dataset, _resources) = context(&combined);
    let combined = edge_grid(&context);
    assert_eq!(cells_of(&combined), cells_of(&overlapping));
    assert_eq!(cells_of(&overlapping).0[0], Some(350.0 / 90.0));
    assert_eq!(overlapping.entities[0].total, Some(300.0));
}

#[test]
fn overlapping_segments_keep_a_following_sample_encountered_before_its_entity() {
    let root = tempfile::tempdir().expect("overlapping directory");
    write_cpu_segment(
        root.path(),
        EDGE_FROM - 100_000_000,
        &[
            (1, EDGE_FROM - 90_000_000, 0),
            (0, EDGE_TO + 30_000_000, 300),
        ],
    );
    write_cpu_segment(
        root.path(),
        EDGE_TO - 80_000_000,
        &[
            (0, EDGE_TO - 80_000_000, 90),
            (0, EDGE_TO - 20_000_000, 150),
        ],
    );
    let result = edge_grid(&posix_context(root.path()));
    assert_eq!(result.entity_count, 1);
    assert_eq!(cells_of(&result).0[11], Some(210.0 / 110.0));
    assert_eq!(cells_of(&result).1[11], Some(210.0 / 110.0));
    assert_eq!(result.entities[0].total, Some(60.0));
    let entity = serde_json::to_value(&result.entities[0]).expect("entity");
    assert_eq!(
        entity["detail_locator"]["at"],
        (EDGE_TO - 20_000_000).to_string()
    );
}

#[test]
fn preceding_edge_spans_allow_exactly_fifteen_minutes() {
    for first in [EDGE_FROM, EDGE_FROM + 50_000_000] {
        for (gap, expected) in [(900_000_000, Some(1.0)), (900_000_001, None)] {
            let payload = edge_payload(&[(0, first - gap, 0), (0, first, 900)]);
            let (context, _dataset, _resources) = context(&payload);
            let result = edge_grid(&context);
            assert_eq!(cells_of(&result).0[0], expected, "gap {gap}");
            assert_eq!(cells_of(&result).1[0], expected, "gap {gap}");
            assert_eq!(result.entities[0].total, None);
        }
    }
}

#[test]
fn following_edge_spans_allow_exactly_fifteen_minutes() {
    for (gap, expected) in [(900_000_000, Some(1.0)), (900_000_001, None)] {
        let last = EDGE_TO - 1;
        let payload = edge_payload(&[(0, last, 0), (0, last + gap, 900)]);
        let (context, _dataset, _resources) = context(&payload);
        let result = edge_grid(&context);
        assert_eq!(cells_of(&result).0[11], expected, "gap {gap}");
        assert_eq!(cells_of(&result).1[11], expected, "gap {gap}");
        assert_eq!(result.entities[0].total, None);
    }
}

#[test]
fn internal_spans_allow_fifteen_minutes_and_resume_after_a_longer_gap() {
    for (gap, expected) in [(900_000_000, Some(1.0)), (900_000_001, None)] {
        let first = EDGE_FROM + 100_000_000;
        let payload = edge_payload(&[
            (0, first, 0),
            (0, first + gap, 900),
            (0, first + gap + 30_000_000, 930),
        ]);
        let (context, _dataset, _resources) = context(&payload);
        let result = edge_grid(&context);
        let (cells, totals) = cells_of(&result);
        assert_eq!(cells[5], expected, "gap {gap}");
        assert_eq!(cells[10], Some(1.0), "valid pair after gap {gap}");
        assert_eq!(cells, totals);
        assert_eq!(result.entities[0].total, Some(930.0));
    }
}

#[test]
fn wide_columns_sum_only_valid_spans_for_entities_groups_and_bands() {
    let root = tempfile::tempdir().expect("fixture directory");
    let rows: Vec<_> = [(0, 2), (1, 1)]
        .into_iter()
        .flat_map(|(cpu, scale)| {
            [
                (0, 0),
                (300_000_000, 300),
                (1_200_000_001, 9_000),
                (1_260_000_001, 9_060),
            ]
            .into_iter()
            .map(move |(offset, value)| (cpu, EDGE_FROM + offset, scale * value))
        })
        .collect();
    write_cpu_segment(root.path(), EDGE_FROM, &rows);
    let context = posix_context(root.path());
    for group in [Vec::new(), vec!["cpu_id".to_owned()]] {
        let query = HeatmapBatchQuery {
            range: TimeRange::new(EDGE_FROM, EDGE_FROM + 3_600_000_000).expect("hour range"),
            items: vec![HeatmapItemQuery {
                view: HeatmapView::Grid {
                    columns: 1,
                    group: group.clone(),
                    type_id: None,
                },
                ..cpu_grid()
            }],
        };
        let result = execute_heatmap_batch(&context, query, &NeverCancelled)
            .expect("wide grid")
            .results
            .remove(0);
        let grid = result.grid.as_ref().expect("grid");
        assert_eq!(grid.totals.cells, vec![Some(3.0)]);
        assert_eq!(grid.others.cells, vec![Some(1.0)]);
        assert_eq!(grid.totals.total, Some(27_180.0));
        assert_eq!(grid.others.total, Some(9_060.0));
        if group.is_empty() {
            assert_eq!(result.entities[0].cells, Some(vec![Some(2.0)]));
        } else {
            assert_eq!(grid.groups[0].cells, vec![Some(2.0)]);
        }
    }
}

#[test]
fn an_earlier_in_range_sample_from_an_overlap_extends_a_valid_cell() {
    let root = tempfile::tempdir().expect("overlapping directory");
    write_cpu_segment(
        root.path(),
        EDGE_FROM - 10_000_000,
        &[
            (1, EDGE_FROM - 10_000_000, 0),
            (0, EDGE_FROM + 200_000_000, 300),
        ],
    );
    write_cpu_segment(
        root.path(),
        EDGE_FROM + 100_000_000,
        &[
            (0, EDGE_FROM + 100_000_000, 100),
            (0, EDGE_FROM + 300_000_000, 400),
        ],
    );
    let context = posix_context(root.path());
    for group in [Vec::new(), vec!["cpu_id".to_owned()]] {
        let mut query = edge_batch();
        query.items[0].view = HeatmapView::Grid {
            columns: 1,
            group,
            type_id: None,
        };
        let result = execute_heatmap_batch(&context, query, &NeverCancelled)
            .expect("overlapping grid")
            .results
            .remove(0);
        assert_eq!(
            result.grid.as_ref().expect("grid").totals.cells,
            vec![Some(1.5)]
        );
        assert_eq!(result.totals_total, Some(300.0));
    }
}

#[test]
fn a_wide_column_accepts_continuous_samples_for_more_than_fifteen_minutes() {
    let rows: Vec<_> = (0..=8)
        .map(|step| (0, EDGE_FROM + step * 300_000_000, step * 300))
        .collect();
    let payload = edge_payload(&rows);
    let (context, _dataset, _resources) = context(&payload);
    let mut query = edge_batch();
    query.range = TimeRange::new(EDGE_FROM, EDGE_FROM + 3_600_000_000).expect("hour range");
    query.items[0].view = HeatmapView::Grid {
        columns: 1,
        group: Vec::new(),
        type_id: None,
    };
    let result = execute_heatmap_batch(&context, query, &NeverCancelled)
        .expect("continuous grid")
        .results
        .remove(0);
    assert_eq!(cells_of(&result), (vec![Some(1.0)], vec![Some(1.0)]));
    assert_eq!(result.entities[0].total, Some(2_400.0));
}

#[test]
fn finished_edge_segments_alone_do_not_make_an_empty_heatmap_immutable() {
    for timestamps in [[EDGE_FROM - 2, EDGE_FROM - 1], [EDGE_TO, EDGE_TO + 1]] {
        let payload = edge_payload(&[(0, timestamps[0], 0), (0, timestamps[1], 1)]);
        let (context, dataset, _resources) = context(&payload);
        let query = kronika_query::validate_heatmap_request(edge_batch()).expect("valid query");
        let execution =
            kronika_query::execute(&context, kronika_query::QueryRequest::Heatmap(query))
                .expect("prepare heatmap");
        assert!(execution.metadata().identity().is_none());
        assert_eq!(
            execution.metadata().stability(),
            kronika_query::QueryStability::Revalidate
        );
        assert_eq!(
            dataset.opens.load(Ordering::Relaxed),
            0,
            "metadata does not decode rows"
        );
    }
}
