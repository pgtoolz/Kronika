use std::sync::Arc;

use hyper::StatusCode;
use kronika_query::{QueryContext, QueryIdentity, execute};

use crate::query_adapter::NativeDataset;
use crate::tests::artifacts::Fixture;

#[test]
fn heatmap_summary_change_invalidates_the_previous_representation_validator() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 90, "postgres"), (200, 101, 30, "postgres")]);
    fixture.finish();
    let query = "from=100&to=999&section=os_process&field=rmem_kb&columns=60&top=10";
    let target = format!("/api/heatmap?{query}");
    let request = kronika_api::parse("/api/heatmap", Some(query))
        .expect("heatmap route")
        .into_query()
        .expect("heatmap query");
    let dataset = NativeDataset::from_root(fixture.root()).expect("recorded dataset");
    let execution = execute(&QueryContext::new(Arc::new(dataset), 0b11, false), request)
        .expect("prepare heatmap");
    let Some(QueryIdentity::SegmentSet {
        resource,
        shape,
        segments,
    }) = execution.metadata().identity()
    else {
        panic!("finished heatmap identity");
    };
    let previous_shape = shape
        .strip_prefix("summary-v1:")
        .expect("summary representation version");
    let previous = super::weak_dataset_etag(resource, previous_shape, segments)
        .expect("previous heatmap validator");
    let current = fixture
        .prepare(&target, None)
        .meta()
        .etag
        .expect("current heatmap validator");
    assert_ne!(current, previous);
    assert_eq!(
        fixture.prepare(&target, Some(&previous)).meta().status,
        StatusCode::OK
    );
    assert_eq!(
        fixture.prepare(&target, Some(&current)).meta().status,
        StatusCode::NOT_MODIFIED
    );
}
