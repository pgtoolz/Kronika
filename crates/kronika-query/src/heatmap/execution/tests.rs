use std::error::Error;

use kronika_reader::ReaderError;

use super::super::HeatmapError;
use crate::QueryError;

#[test]
fn rss_mean_uses_one_recorded_snapshot_denominator_for_every_entity() {
    let mut mean = super::RssMean::default();
    assert_eq!(mean.mean(super::EntityId(0)), None);

    mean.observe(super::EntityId(0), 100, 90.0);
    mean.observe(super::EntityId(1), 100, 120.0);
    mean.observe(super::EntityId(0), 200, 90.0);
    mean.observe(super::EntityId(7), 900, 0.0);

    assert_eq!(mean.mean(super::EntityId(0)), Some(60.0));
    assert_eq!(mean.mean(super::EntityId(1)), Some(40.0));
    assert_eq!(mean.mean(super::EntityId(7)), Some(0.0));
}

#[test]
fn storage_error_keeps_its_source_chain() {
    let reader = HeatmapError::storage(
        0,
        QueryError::from(ReaderError::Io(std::io::Error::from(
            std::io::ErrorKind::Interrupted,
        ))),
    )
    .into_query();
    assert!(reader.source_changed_during_read());

    let remote = HeatmapError::storage(0, RemoteError).into_query();
    let heatmap = remote.source().expect("heatmap wrapper");
    assert!(heatmap.source().is_some_and(<dyn Error>::is::<RemoteError>));
}

#[derive(Debug)]
struct RemoteError;

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("remote object failed")
    }
}

impl Error for RemoteError {}
