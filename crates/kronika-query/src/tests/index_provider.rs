use std::sync::Arc;

use kronika_index::{HealthPoint, Index, IndexError, SeriesBlock, SeriesKey, TargetedIndex};
use kronika_layout::SegmentId;
use kronika_reader::{SegmentKind, SegmentSection};

use super::{IndexProvider as _, MemoryIndexProvider};
use crate::{DatasetSegment, OpaqueCapture, QueryError};

fn encoded_index() -> Vec<u8> {
    Index {
        blocks: vec![
            SeriesBlock::OsHealth(vec![HealthPoint {
                timestamp: 42,
                value: Some(95),
            }]),
            SeriesBlock::OverallHealth(vec![HealthPoint {
                timestamp: 42,
                value: Some(91),
            }]),
        ],
    }
    .encode()
    .expect("encode current index")
}

fn segment(id: i64, kind: SegmentKind) -> DatasetSegment {
    DatasetSegment::new(
        OpaqueCapture::new(()),
        id,
        kind,
        42,
        42,
        None,
        Arc::<[SegmentSection]>::from([]),
    )
}

fn unreadable_io(error: &QueryError) -> Option<&std::io::Error> {
    let QueryError::Unreadable(error) = error else {
        return None;
    };
    error.downcast_ref()
}

#[test]
fn memory_provider_matches_targeted_decode_and_reuses_encoded_bytes() {
    let bytes = encoded_index();
    let original_ptr = bytes.as_ptr();
    let expected = Index::decode_target(&bytes, &[SeriesKey::OS_HEALTH]).expect("targeted index");
    let provider = MemoryIndexProvider::new(SegmentId::new(42).expect("segment id"), bytes)
        .expect("memory provider");
    let clone = provider.clone();

    assert_eq!(provider.bytes.0.as_ptr(), original_ptr);
    assert_eq!(clone.bytes.0.as_ptr(), original_ptr);
    for candidate in [&provider, &clone, &provider] {
        assert_eq!(
            candidate
                .load(
                    &segment(42, SegmentKind::Finished),
                    "health",
                    &[SeriesKey::OS_HEALTH]
                )
                .expect("selected index")
                .index,
            expected
        );
        assert_eq!(candidate.bytes.0.as_ptr(), original_ptr);
    }
}

#[test]
fn memory_provider_rejects_foreign_active_and_incomplete_resources() {
    let provider =
        MemoryIndexProvider::new(SegmentId::new(42).expect("segment id"), encoded_index())
            .expect("memory provider");

    assert!(matches!(
        provider.load(
            &segment(43, SegmentKind::Finished),
            "health",
            &[SeriesKey::OS_HEALTH]
        ),
        Err(QueryError::NoSuchSegment)
    ));
    let active = provider
        .load(
            &segment(42, SegmentKind::Active),
            "health",
            &[SeriesKey::OS_HEALTH],
        )
        .expect_err("active segment must be rejected");
    assert_eq!(
        unreadable_io(&active).map(std::io::Error::kind),
        Some(std::io::ErrorKind::InvalidInput)
    );
    let missing = provider
        .load(
            &segment(42, SegmentKind::Finished),
            "health",
            &[SeriesKey::POSTGRES_HEALTH, SeriesKey::OVERALL_HEALTH],
        )
        .expect("optional PostgreSQL health may be absent");
    assert_eq!(
        missing.index,
        TargetedIndex {
            checksum: Index::decode_target(&encoded_index(), &[])
                .expect("checksum")
                .checksum,
            blocks: vec![SeriesBlock::OverallHealth(vec![HealthPoint {
                timestamp: 42,
                value: Some(91),
            }])],
        }
    );
    let missing = provider
        .load(
            &segment(42, SegmentKind::Finished),
            "health",
            &[SeriesKey {
                kind: kronika_index::SeriesKind::PgTransactionsPerSecond,
                type_id: 1_005_004,
            }],
        )
        .expect_err("required block must be present");
    assert_eq!(
        unreadable_io(&missing).map(std::io::Error::kind),
        Some(std::io::ErrorKind::InvalidData)
    );
}

#[test]
fn memory_provider_rejects_invalid_or_oversized_containers_at_construction() {
    let segment_id = SegmentId::new(42).expect("segment id");
    let mut bad_magic = encoded_index();
    bad_magic[0] ^= 1;
    assert!(matches!(
        MemoryIndexProvider::new(segment_id, bad_magic),
        Err(IndexError::BadMagic)
    ));

    let mut bad_checksum = encoded_index();
    *bad_checksum.last_mut().expect("index body") ^= 1;
    assert!(matches!(
        MemoryIndexProvider::new(segment_id, bad_checksum),
        Err(IndexError::BadChecksum)
    ));
    assert!(matches!(
        MemoryIndexProvider::new(segment_id, vec![0; 8 * 1024 * 1024 + 1]),
        Err(IndexError::TooLarge)
    ));
}
