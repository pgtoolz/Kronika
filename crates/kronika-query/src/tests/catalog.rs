use std::sync::Arc;

use kronika_reader::{Segment, SegmentKind, SegmentSection};
use kronika_registry::DICT_STRINGS_TYPE_ID;

use super::catalog_facts;
use crate::{
    CapturedCatalog, CatalogRequest, DatasetListing, DatasetSegment, OpaqueCapture, QueryContext,
    QueryDataset, QueryError, SegmentBounds, SegmentSelection, Window,
};

#[derive(Debug)]
struct FixedDataset {
    segments: Vec<DatasetSegment>,
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
            SegmentSelection::new(SegmentBounds::inclusive(Some(50), Some(400)))
        );
        Ok(DatasetListing {
            segments: self.0.segments.clone(),
            warnings: Vec::new(),
        })
    }
}

impl QueryDataset for FixedDataset {
    fn catalog(&self) -> Result<Box<dyn CapturedCatalog + '_>, QueryError> {
        Ok(Box::new(FixedCatalog(self)))
    }

    fn segment(&self, _id: i64) -> Result<DatasetListing, QueryError> {
        unreachable!("catalog facts do not select one segment")
    }

    fn open(&self, _segment: &DatasetSegment) -> Result<Segment, QueryError> {
        unreachable!("catalog facts do not open segment bodies")
    }

    fn at_active_position(
        &self,
        _segment: &DatasetSegment,
        _position: u64,
    ) -> Result<DatasetSegment, QueryError> {
        unreachable!("catalog facts do not pin active data")
    }
}

fn segment(id: i64, min_ts: i64, max_ts: i64, rows: u64, bytes: u64) -> DatasetSegment {
    DatasetSegment::new(
        OpaqueCapture::new(id),
        id,
        SegmentKind::Finished,
        min_ts,
        max_ts,
        None,
        Arc::from([
            SegmentSection {
                type_id: 1_100_001,
                rows,
                bytes,
            },
            SegmentSection {
                type_id: DICT_STRINGS_TYPE_ID,
                rows: 1,
                bytes: 10,
            },
        ]),
    )
}

#[test]
fn typed_facts_preserve_range_totals_fields_and_internal_section_filtering() {
    let dataset = Arc::new(FixedDataset {
        segments: vec![segment(10, 100, 200, 2, 30), segment(20, 300, 350, 3, 40)],
    });
    let context = QueryContext::new(dataset, 0, false);

    let facts = catalog_facts(
        &context,
        CatalogRequest {
            window: Window {
                from: Some(50),
                to: Some(400),
            },
        },
    )
    .expect("typed catalog facts");

    assert_eq!(facts.recorded_range, Some((100, 350)));
    assert_eq!(facts.sections.len(), 1);
    let process = &facts.sections[0];
    assert_eq!(process.logical_name, "os_process");
    assert_eq!(process.source_family, Some("os"));
    assert_eq!((process.rows, process.bytes), (5, 70));
    assert!(
        process
            .fields
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(process.fields.iter().any(|field| {
        field.name == "rmem_kb" && field.class == "gauge" && field.unit == Some("kibibytes")
    }));
}
