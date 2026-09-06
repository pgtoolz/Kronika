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
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{StrId, Ts};
use kronika_store::{
    EmbeddedResource, EmbeddedSource, ImmutableSegmentSource, ResourceCatalog, ResourceError,
    ResourceListing, SegmentResource, SharedSegmentBytes,
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
