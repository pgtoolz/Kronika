use std::sync::Arc;

// Dependencies exercised by integration targets; anchor them for this crate's
// unit-test target under the workspace's unused-dependency lint.
use kronika_layout as _;
use kronika_reader::{Segment, SegmentKind, SegmentSection};
use tempfile as _;

use crate::{
    CapturedCatalog, CatalogRequest, DatasetListing, DatasetSegment, DatasetWarning, OpaqueCapture,
    QueryContext, QueryDataset, QueryError, QueryRequest, QuerySink, SOURCE_OS, SOURCE_POSTGRESQL,
    SegmentBounds, SegmentSelection, Window, execute, source_bit,
};

#[test]
fn registered_source_families_include_wal_storage_but_not_metadata() {
    assert_eq!(source_bit(1_020_001), Some(SOURCE_POSTGRESQL));
    assert_eq!(source_bit(1_021_002), None);
    assert_eq!(source_bit(1_107_001), Some(SOURCE_OS));
}

#[derive(Debug)]
struct FixedDataset {
    segments: Vec<DatasetSegment>,
    warnings: Vec<DatasetWarning>,
}

#[derive(Debug)]
struct FixedCatalog<'a>(&'a FixedDataset);

impl CapturedCatalog for FixedCatalog<'_> {
    fn ranges(&self) -> &[(i64, i64)] {
        &[]
    }

    fn segments(&self, selection: SegmentSelection) -> Result<DatasetListing, QueryError> {
        assert_eq!(
            selection,
            SegmentSelection::new(SegmentBounds::inclusive(Some(10), Some(20)))
        );
        Ok(DatasetListing {
            segments: self.0.segments.clone(),
            warnings: self.0.warnings.clone(),
        })
    }
}

impl QueryDataset for FixedDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        Ok(Box::new(FixedCatalog(self)))
    }

    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("catalog test does not select an exact segment")
    }

    fn open(&self, _segment: &DatasetSegment) -> Result<Segment, QueryError> {
        unreachable!("catalog test does not open bodies")
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        unreachable!("catalog test does not pin active bytes")
    }
}

#[derive(Default)]
struct Records(Vec<Vec<u8>>);

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0.push(bytes);
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

#[test]
fn catalog_records_are_exact_and_storage_neutral() {
    let dataset = Arc::new(FixedDataset {
        segments: vec![DatasetSegment::new(
            OpaqueCapture::new("capture"),
            11,
            SegmentKind::Finished,
            10,
            20,
            None,
            Arc::from([SegmentSection {
                type_id: 1_107_001,
                rows: 2,
                bytes: 30,
            }]),
        )],
        warnings: Vec::new(),
    });
    let context = QueryContext::new(dataset, 0b11, false);
    let execution = execute(
        &context,
        QueryRequest::Catalog(CatalogRequest {
            window: Window {
                from: Some(10),
                to: Some(20),
            },
        }),
    )
    .expect("prepare catalog query");
    let mut records = Records::default();
    execution.stream(&mut records).expect("stream catalog");

    assert_eq!(
        records.0,
        [
            concat!(
                "{\"demo\":null,\"from\":\"10\",\"record\":\"catalog\",",
                "\"source_families\":[{\"configured\":true,\"metrics_present\":true,",
                "\"name\":\"os\",\"present\":true},{\"configured\":true,",
                "\"metrics_present\":false,\"name\":\"postgresql\",",
                "\"present\":false}],\"to\":\"20\"}\n"
            )
            .as_bytes()
            .to_vec(),
            concat!(
                "{\"id\":\"11\",\"max_ts\":\"20\",\"min_ts\":\"10\",",
                "\"record\":\"finished_segment\",\"sections\":[{\"bytes\":\"30\",",
                "\"implementation\":null,\"logical_name\":\"os_psi\",",
                "\"physical_name\":\"os_psi\",\"rows\":\"2\",",
                "\"source_family\":\"os\",\"type_id\":\"1107001\"}]}\n"
            )
            .as_bytes()
            .to_vec(),
        ]
    );
}
