use std::sync::Arc;

use kronika_reader::Segment;

use super::{HOUR, hours_of_ranges, latest_hour, overlaps_window};
use crate::{
    CapturedCatalog, DatasetListing, DatasetSegment, HourPart, HourRequest, QueryContext,
    QueryDataset, QueryError, QueryRequest, QuerySink, QueryStability, SegmentSelection, Window,
    execute,
};

#[derive(Debug)]
struct EmptyHourDataset {
    ranges: Vec<(i64, i64)>,
}

#[derive(Debug)]
struct EmptyHourCatalog<'a>(&'a EmptyHourDataset);

impl CapturedCatalog for EmptyHourCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        &self.0.ranges
    }

    fn segments(&self, _selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        Ok(DatasetListing {
            segments: Vec::new(),
            warnings: Vec::new(),
        })
    }
}

impl QueryDataset for EmptyHourDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        Ok(Box::new(EmptyHourCatalog(self)))
    }

    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("hour inventory does not select one explicit segment")
    }

    fn open(&self, _segment: &DatasetSegment) -> Result<Segment, QueryError> {
        unreachable!("the empty hour has no segment body")
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        unreachable!("the empty hour has no active segment")
    }
}

struct ControlSink {
    records: Vec<Vec<u8>>,
    cancelled: bool,
    connected: bool,
}

impl QuerySink for ControlSink {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.records.push(bytes);
        self.connected
    }

    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

fn empty_base_execution() -> crate::QueryExecution {
    let context = QueryContext::new(
        Arc::new(EmptyHourDataset {
            ranges: vec![(10, 20)],
        }),
        0b11,
        false,
    );
    execute(
        &context,
        QueryRequest::Hour(HourRequest {
            window: Window {
                from: Some(10),
                to: Some(20),
            },
            series: None,
            part: HourPart::Base,
            segments: None,
            active: None,
        }),
    )
    .expect("prepare empty base hour")
}

#[test]
fn long_segments_include_every_intersected_hour() {
    assert_eq!(
        hours_of_ranges([(HOUR + 1, 3 * HOUR + 1), (2 * HOUR, 2 * HOUR)]),
        [HOUR, 2 * HOUR, 3 * HOUR],
    );
}

#[test]
fn exact_and_maximum_boundaries_are_safe() {
    let maximum_hour = i64::MAX.div_euclid(HOUR) * HOUR;
    assert_eq!(hours_of_ranges([(HOUR, HOUR)]), [HOUR]);
    assert_eq!(hours_of_ranges([(i64::MAX, i64::MAX)]), [maximum_hour]);
    assert_eq!(latest_hour(&[maximum_hour]).to, Some(i64::MAX));
    assert!(hours_of_ranges([(3, 2)]).is_empty());
}

#[test]
fn selected_segments_use_inclusive_window_bounds() {
    let window = Window {
        from: Some(100),
        to: Some(200),
    };
    assert!(!overlaps_window(0, 99, window));
    assert!(overlaps_window(0, 100, window));
    assert!(overlaps_window(150, 160, window));
    assert!(overlaps_window(200, 300, window));
    assert!(!overlaps_window(201, 300, window));
    assert!(overlaps_window(0, 300, window));
    assert!(overlaps_window(100, 200, Window::default()));
}

#[test]
fn empty_base_hour_records_are_exact() {
    let execution = empty_base_execution();
    assert_eq!(execution.metadata().stability(), QueryStability::Revalidate);
    assert!(execution.metadata().identity().is_none());
    let mut sink = ControlSink {
        records: Vec::new(),
        cancelled: false,
        connected: true,
    };
    execution.stream(&mut sink).expect("stream empty base hour");
    assert_eq!(
        sink.records,
        [
            b"{\"available_hours\":[\"0\"],\"from\":\"10\",\"record\":\"hour\",\"to\":\"20\"}\n"
                .to_vec(),
            concat!(
                "{\"demo\":null,\"from\":\"10\",\"record\":\"catalog\",",
                "\"source_families\":[{\"configured\":true,\"metrics_present\":false,",
                "\"name\":\"os\",\"present\":false},{\"configured\":true,",
                "\"metrics_present\":false,\"name\":\"postgresql\",",
                "\"present\":false}],\"to\":\"20\"}\n",
            )
            .as_bytes()
            .to_vec(),
        ]
    );
}

#[test]
fn hour_honors_initial_cancellation_and_first_record_disconnect() {
    let mut cancelled = ControlSink {
        records: Vec::new(),
        cancelled: true,
        connected: true,
    };
    empty_base_execution()
        .stream(&mut cancelled)
        .expect("cancelled hour is a clean stop");
    assert!(cancelled.records.is_empty());

    let mut disconnected = ControlSink {
        records: Vec::new(),
        cancelled: false,
        connected: false,
    };
    empty_base_execution()
        .stream(&mut disconnected)
        .expect("disconnected hour is a clean stop");
    assert_eq!(disconnected.records.len(), 1);
}

#[test]
fn hour_series_debug_keeps_the_validator_shape_name() {
    let request = crate::HourSeriesRequest {
        section: "os_cpu".to_owned(),
        fields: vec!["user".to_owned()],
        filters: Vec::new(),
        type_id: Some(1),
        group: None,
        scope: crate::StatementScope::All,
    };
    assert_eq!(
        format!("{request:?}"),
        "SeriesRequest { section: \"os_cpu\", fields: [\"user\"], filters: [], type_id: Some(1), group: None, scope: All }"
    );
}

#[derive(Debug)]
struct PinnedOrderDataset;

fn order_descriptor(id: i64, min_ts: i64, active: bool) -> DatasetSegment {
    DatasetSegment::new(
        crate::OpaqueCapture::new(()),
        id,
        if active {
            kronika_reader::SegmentKind::Active
        } else {
            kronika_reader::SegmentKind::Finished
        },
        min_ts,
        60,
        active.then_some(7),
        Arc::from([]),
    )
}

impl CapturedCatalog for PinnedOrderDataset {
    fn ranges(&self) -> &[(i64, i64)] {
        &[(0, 60)]
    }
    fn segments(&self, _selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        Ok(DatasetListing {
            segments: vec![
                order_descriptor(1, 0, false),
                order_descriptor(2, 10, true),
                order_descriptor(3, 20, false),
            ],
            warnings: Vec::new(),
        })
    }
}

impl QueryDataset for PinnedOrderDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        Ok(Box::new(Self))
    }
    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("hour preparation uses catalog")
    }
    fn open(&self, _segment: &DatasetSegment) -> Result<Segment, QueryError> {
        unreachable!("preparation does not open segment bodies")
    }
    fn at_active_position(
        &self,
        segment: &DatasetSegment,
        position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        assert_eq!((segment.id(), position), (2, 7));
        Ok(order_descriptor(2, 50, true))
    }
}

#[test]
fn pinned_prefix_restores_time_order_after_validating_listed_ids() {
    let request = |ids| HourRequest {
        window: Window {
            from: Some(0),
            to: Some(60),
        },
        series: None,
        part: HourPart::Lanes,
        segments: Some(ids),
        active: Some(crate::ActiveCursor {
            segment_id: 2,
            wal_position: 7,
        }),
    };
    let prepared = super::prepare(
        Arc::new(PinnedOrderDataset),
        None,
        request(vec![1, 2, 3]),
        1,
        false,
    )
    .expect("valid pinned prefix");
    assert_eq!(
        prepared
            .segments
            .iter()
            .map(|segment| (segment.id(), segment.min_ts()))
            .collect::<Vec<_>>(),
        [(1, 0), (3, 20), (2, 50)]
    );
    assert!(matches!(
        super::prepare(
            Arc::new(PinnedOrderDataset),
            None,
            request(vec![1, 3, 2]),
            1,
            false
        ),
        Err(QueryError::BadCursor)
    ));
}
