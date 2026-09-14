use super::*;

const HOUR_US: i64 = 3_600_000_000;
const FUTURE_EVENT: i64 = HEATMAP_TO + 4 * HOUR_US;

// The chosen hour is emitted before unrelated derived-index records. Stop at
// that header: this regression exercises selection, not index construction.
fn hour_header(dataset: Arc<dyn QueryDataset>, request: HourRequest) -> Vec<u8> {
    struct Header(Vec<u8>);
    impl QuerySink for Header {
        fn record(&mut self, bytes: Vec<u8>) -> bool {
            self.0 = bytes;
            false
        }
        fn cancelled(&self) -> bool {
            false
        }
    }
    let context = QueryContext::new(dataset, 0b11, false);
    let mut sink = Header(Vec::new());
    execute(&context, QueryRequest::Hour(request), &|| false)
        .expect("prepare hour")
        .stream(&mut sink)
        .expect("hour header");
    sink.0
}

#[expect(
    clippy::too_many_lines,
    reason = "same capture validates all nine finders, explicit time, hour and retained events"
)]
fn assert_metric_defaults(dataset: Arc<dyn QueryDataset>) {
    let context = QueryContext::new(Arc::clone(&dataset), 0b11, false);
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
        let result = finder_result_json(&context, surface);
        assert_eq!(result["as_of"], HEATMAP_TO, "{surface:?}");
        assert_eq!(
            result["rows"].as_array().map(Vec::len),
            Some(1),
            "{surface:?}"
        );
    }
    let mut query = finder_query(FinderSurface::Activity);
    query.point = SnapshotPoint::At(FUTURE_EVENT);
    assert!(
        execute_plain(&context, &query, &|| false)
            .expect("explicit future")
            .rows
            .is_empty(),
        "explicit future At must preserve the stale cutoff"
    );
    query.point = SnapshotPoint::At(HEATMAP_TO);
    assert_eq!(
        execute_plain(&context, &query, &|| false)
            .expect("explicit metric time")
            .rows
            .len(),
        1,
        "explicit historical At keeps its observation"
    );
    query.point = SnapshotPoint::LatestRecorded;
    assert!(
        matches!(
            execute_plain(&context, &query, &|| true),
            Err(kronika_query::QueryError::Cancelled)
        ),
        "cancelled finder must stop before discovery"
    );
    let records = ndjson(&hour_header(
        Arc::clone(&dataset),
        HourRequest {
            window: Window::default(),
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        },
    ));
    let hour = records
        .iter()
        .find(|record| record["record"] == "hour")
        .expect("hour header");
    assert_eq!(
        hour["from"],
        (HEATMAP_TO.div_euclid(HOUR_US) * HOUR_US).to_string(),
        "default hour follows the common metric observation"
    );
    let explicit = ndjson(&hour_header(
        Arc::clone(&dataset),
        HourRequest {
            window: Window {
                from: Some(FUTURE_EVENT),
                to: Some(FUTURE_EVENT + 1),
            },
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        },
    ));
    assert_eq!(
        explicit[0]["from"],
        FUTURE_EVENT.to_string(),
        "explicit hour is never moved to metric time"
    );
    let facts =
        kronika_query::catalog_facts(&context, CatalogRequest::default()).expect("whole catalog");
    assert_eq!(
        facts.recorded_range.map(|range| range.1),
        Some(FUTURE_EVENT),
        "whole-data range must still include the future event"
    );
    let events = events_result(
        dataset,
        EventsQuery::normalize(
            TimeRange::new(FUTURE_EVENT, FUTURE_EVENT + 1).expect("event window"),
            Some(vec!["pg_log_temp_files".to_owned()]),
            EventsRepresentation::Occurrences,
            10,
        )
        .expect("events request"),
    );
    let occurrences = match events {
        EventsResult::Occurrences { occurrences, .. } => Some(occurrences),
        EventsResult::Groups { .. } => None,
    }
    .expect("occurrences response");
    assert_eq!(occurrences.len(), 1, "future event remains queryable");
}

#[test]
fn metric_clock_ignores_future_events_for_all_nine_finders_in_wal_zms_and_embedded() {
    let directory = tempfile::tempdir().expect("recording");
    let id = SegmentId::new(SEGMENT_ID).expect("segment id");
    let payload = write_heatmap_fixture_observed(
        directory.path(),
        id,
        None,
        42,
        Some(FUTURE_EVENT),
        |root| {
            assert_metric_defaults(Arc::new(
                query_adapter::NativeDataset::from_root(root).expect("active reader"),
            ));
        },
    );
    assert_metric_defaults(Arc::new(
        query_adapter::NativeDataset::from_root(directory.path()).expect("sealed reader"),
    ));
    let embedded =
        EmbeddedSource::from_owned(id, payload.to_vec(), payload.len() as u64).expect("embedded");
    assert_metric_defaults(Arc::new(FinishedDataset::new(embedded)));
}

#[test]
fn metric_clock_does_not_resurrect_a_stale_source_and_explicit_at_still_works() {
    let directory = tempfile::tempdir().expect("recording");
    let id = SegmentId::new(SEGMENT_ID).expect("segment id");
    drop(write_heatmap_fixture_observed(
        directory.path(),
        id,
        None,
        42,
        Some(FUTURE_EVENT),
        |_| {},
    ));
    // Only another source has new observations. An empty/failed activity poll
    // writes no marker; its old rows must still obey the common age boundary.
    let later = HEATMAP_TO + 1_000_000_000;
    write_process_segment(
        directory.path(),
        SegmentId::new(SEGMENT_ID + 1).expect("later id"),
        later,
        99,
        10,
    );
    let context = QueryContext::new(
        Arc::new(query_adapter::NativeDataset::from_root(directory.path()).expect("reader")),
        0b11,
        false,
    );
    for surface in [
        FinderSurface::Activity,
        FinderSurface::Locks,
        FinderSurface::Vacuum,
        FinderSurface::Tables,
        FinderSurface::Indexes,
        FinderSurface::Databases,
        FinderSurface::Statements,
        FinderSurface::Plans,
    ] {
        assert_eq!(
            finder_result_json(&context, surface)["rows"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "{surface:?}"
        );
    }
    let mut query = finder_query(FinderSurface::Activity);
    query.point = SnapshotPoint::At(HEATMAP_TO);
    assert_eq!(
        execute_plain(&context, &query, &|| false)
            .expect("historical at")
            .rows
            .len(),
        1
    );
}

#[test]
fn metric_clock_event_only_dataset_keeps_default_hour_and_has_no_current_metrics() {
    let directory = tempfile::tempdir().expect("recording");
    drop(write_events_fixture(
        directory.path(),
        SegmentId::new(SEGMENT_ID).expect("id"),
    ));
    let dataset: Arc<dyn QueryDataset> =
        Arc::new(query_adapter::NativeDataset::from_root(directory.path()).expect("reader"));
    let context = QueryContext::new(Arc::clone(&dataset), 0b10, false);
    assert!(
        execute_plain(&context, &finder_query(FinderSurface::Activity), &|| false)
            .expect("no metrics")
            .rows
            .is_empty()
    );
    let records = ndjson(&hour_header(
        dataset,
        HourRequest {
            window: Window::default(),
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        },
    ));
    let hour = records
        .iter()
        .find(|record| record["record"] == "hour")
        .expect("hour");
    assert_eq!(
        hour["from"],
        (SEGMENT_ID.div_euclid(HOUR_US) * HOUR_US).to_string()
    );
}

#[test]
fn metric_clock_default_hour_prunes_older_bodies_once_observation_reaches_bound() {
    let directory = tempfile::tempdir().expect("recording");
    drop(write_heatmap_fixture(
        directory.path(),
        SegmentId::new(SEGMENT_ID).expect("first id"),
    ));
    let later = HEATMAP_TO + HOUR_US;
    write_process_segment(
        directory.path(),
        SegmentId::new(SEGMENT_ID + 1).expect("second id"),
        later,
        99,
        10,
    );
    let dataset = Arc::new(CountingRowDetailDataset {
        inner: FinishedDataset::new(PosixSource::open(directory.path()).expect("source")),
        opens: AtomicUsize::new(0),
    });
    let context = QueryContext::new(
        Arc::<CountingRowDetailDataset>::clone(&dataset),
        0b11,
        false,
    );
    let _prepared = execute(
        &context,
        QueryRequest::Hour(HourRequest {
            window: Window::default(),
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        }),
        &|| false,
    )
    .expect("prepare default hour");
    assert_eq!(
        dataset.opens.load(Ordering::Relaxed),
        1,
        "horizon opens only newest observation body"
    );
}

#[test]
fn metric_clock_keeps_recorded_cadence_when_a_different_source_advances() {
    let directory = tempfile::tempdir().expect("recording");
    drop(write_heatmap_fixture_observed(
        directory.path(),
        SegmentId::new(SEGMENT_ID).expect("id"),
        None,
        42,
        Some(FUTURE_EVENT),
        |_| {},
    ));
    let root = DataRoot::open(directory.path()).expect("root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("journal");
    let mut interner = Interner::new(DictLimits::default());
    let label = fixture_label(&mut interner, b"later observation");
    let ts = Ts(HEATMAP_TO + 100_000_000);
    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadataV3 {
            ts,
            hostname: None,
            kernel_version: None,
            environment: Some(0),
            clock_ticks_per_sec: Some(100),
            page_size_bytes: Some(4096),
            boot_id: None,
            btime: None,
            os_enabled: true,
            postgresql_processes_shared: false,
            postgresql_enabled: true,
            postgresql_interval_seconds: 60,
            postgresql_effective_cpus: None,
        })
        .expect("recorded cadence");
    buffers
        .push(parity_process(ts.0, label))
        .expect("other source observation");
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode")
        .expect("nonempty");
    journal
        .append(SegmentId::new(SEGMENT_ID + 1).expect("later id"), &part)
        .expect("append");
    let context = QueryContext::new(
        Arc::new(query_adapter::NativeDataset::from_root(directory.path()).expect("reader")),
        0b11,
        false,
    );
    let query = finder_query(FinderSurface::Activity);
    assert_eq!(
        execute_plain(&context, &query, &|| false)
            .expect("cadenced snapshot")
            .as_of,
        Some(HEATMAP_TO),
        "100s-old activity fits recorded 60s cadence (150s lookback), not default 75s"
    );
}

#[derive(Debug)]
struct ClockDataset {
    inner: FinishedDataset<PosixSource>,
    listings: std::sync::Mutex<Vec<(kronika_query::SegmentBounds, usize)>>,
    cancel_on_open: bool,
    cancelled: std::sync::atomic::AtomicBool,
}

#[derive(Debug)]
struct ClockCatalog<'a> {
    inner: Box<dyn kronika_query::CapturedCatalog + 'a>,
    listings: &'a std::sync::Mutex<Vec<(kronika_query::SegmentBounds, usize)>>,
}
impl kronika_query::CapturedCatalog for ClockCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        self.inner.ranges()
    }
    fn segments(
        &self,
        selection: kronika_query::SegmentSelection,
    ) -> Result<kronika_query::DatasetListing, kronika_query::QueryError> {
        let bounds = selection.bounds;
        let listing = self.inner.segments(selection)?;
        self.listings
            .lock()
            .map_err(|_poison| {
                kronika_query::QueryError::Unreadable(Box::new(std::io::Error::other(
                    "listing spy poisoned",
                )))
            })?
            .push((bounds, listing.segments.len()));
        Ok(listing)
    }
}
impl QueryDataset for ClockDataset {
    fn catalog(
        &self,
    ) -> Result<Box<dyn kronika_query::CapturedCatalog + '_>, kronika_query::QueryError> {
        Ok(Box::new(ClockCatalog {
            inner: self.inner.catalog()?,
            listings: &self.listings,
        }))
    }
    fn segment(&self, id: i64) -> Result<kronika_query::DatasetListing, kronika_query::QueryError> {
        self.inner.segment(id)
    }
    fn open(
        &self,
        segment: &kronika_query::DatasetSegment,
    ) -> Result<kronika_reader::Segment, kronika_query::QueryError> {
        let opened = self.inner.open(segment)?;
        if self.cancel_on_open {
            self.cancelled.store(true, Ordering::Relaxed);
        }
        Ok(opened)
    }
    fn at_active_position(
        &self,
        segment: &kronika_query::DatasetSegment,
        position: u64,
    ) -> Result<kronika_query::DatasetSegment, kronika_query::QueryError> {
        self.inner.at_active_position(segment, position)
    }
}

#[test]
fn metric_clock_catalog_scope_is_bounded_and_cancel_reaches_in_progress_hour_prepare() {
    let directory = tempfile::tempdir().expect("recording");
    for index in 0..12 {
        write_process_segment(
            directory.path(),
            SegmentId::new(SEGMENT_ID + index).expect("id"),
            HEATMAP_TO + index * HOUR_US,
            99,
            index,
        );
    }
    for cancel_on_open in [false, true] {
        let dataset = Arc::new(ClockDataset {
            inner: FinishedDataset::new(PosixSource::open(directory.path()).expect("source")),
            listings: std::sync::Mutex::new(Vec::new()),
            cancel_on_open,
            cancelled: std::sync::atomic::AtomicBool::new(false),
        });
        let context = QueryContext::new(Arc::<ClockDataset>::clone(&dataset), 0b11, false);
        let result = execute(
            &context,
            QueryRequest::Hour(HourRequest {
                window: Window::default(),
                series: None,
                part: HourPart::Base,
                segments: None,
                active: None,
            }),
            &|| dataset.cancelled.load(Ordering::Relaxed),
        );
        let listings = dataset.listings.lock().expect("listing spy").clone();
        assert!(!listings.is_empty(), "search began before cancellation");
        assert_eq!(
            listings[0],
            (
                kronika_query::SegmentBounds::inclusive(
                    Some(HEATMAP_TO + 11 * HOUR_US),
                    Some(HEATMAP_TO + 11 * HOUR_US)
                ),
                1
            ),
            "only top catalog was materialized for horizon"
        );
        assert!(
            listings.iter().all(|(_, count)| *count == 1),
            "old full catalogs remain unopened in every selection"
        );
        if cancel_on_open {
            assert!(
                matches!(result, Err(kronika_query::QueryError::Cancelled)),
                "cancel reached timestamp read after a real segment open"
            );
            assert_eq!(listings.len(), 1, "cancel does not start another listing");
        } else {
            assert!(result.is_ok(), "default hour preparation completes");
        }
    }
}

#[test]
fn metric_clock_checks_other_mixed_candidates_after_finding_initial_sample() {
    let directory = tempfile::tempdir().expect("recording");
    let newer = HEATMAP_TO + 1_000_000;
    write_process_segment(
        directory.path(),
        SegmentId::new(SEGMENT_ID).expect("first id"),
        newer,
        99,
        1,
    );
    drop(write_heatmap_fixture_observed(
        directory.path(),
        SegmentId::new(SEGMENT_ID + 1).expect("mixed id"),
        None,
        42,
        Some(FUTURE_EVENT),
        |_| {},
    ));
    let context = QueryContext::new(
        Arc::new(query_adapter::NativeDataset::from_root(directory.path()).expect("reader")),
        0b11,
        false,
    );
    let result = execute_processes(&context, &finder_query(FinderSurface::Processes), &|| false)
        .expect("common maximum");
    assert_eq!(
        result.as_of,
        Some(newer),
        "larger metric sample exists outside segment with largest event maximum"
    );
}
