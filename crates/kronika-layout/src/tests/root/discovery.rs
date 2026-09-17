//! Discovery.

use super::*;
use crate::LimitKind;

#[test]
fn strict_scan_sorts_numeric_ids_and_associates_indexs() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
    let later = address(1_709_164_802_000_000);
    let earlier = address(1_709_164_801_000_000);
    for item in [later, earlier] {
        let mut temp = owner.create_zms_temp(item).unwrap();
        temp.file_mut().write_all(b"ZMS").unwrap();
        temp.publish().unwrap();
    }

    let snapshot = root.scan(LayoutLimits::default()).unwrap();
    assert_eq!(
        snapshot
            .segments
            .iter()
            .map(|segment| segment.address.id)
            .collect::<Vec<_>>(),
        vec![earlier.id, later.id]
    );
}

#[test]
fn a_day_with_more_than_192_segments_is_valid_when_within_explicit_limits() {
    let directory = tempfile::tempdir().unwrap();
    let day = directory.path().join("2024/02/29");
    std::fs::create_dir_all(&day).unwrap();
    let midnight = 1_709_164_800_000_000_i64;
    for offset in 0..256_i64 {
        std::fs::write(day.join(format!("{}.zms", midnight + offset)), b"ZMS").unwrap();
    }

    let snapshot = DataRoot::open(directory.path())
        .unwrap()
        .scan(LayoutLimits::default())
        .unwrap();
    assert_eq!(snapshot.segments.len(), 256);
    assert_eq!(snapshot.days, vec![UtcDay::new(2024, 2, 29).unwrap()]);
}

#[test]
fn traversal_returns_no_partial_result_at_a_limit() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
    for value in [1_709_164_801_000_000, 1_709_164_802_000_000] {
        let mut temp = owner.create_zms_temp(address(value)).unwrap();
        temp.file_mut().write_all(b"ZMS").unwrap();
        temp.publish().unwrap();
    }
    let limits = LayoutLimits {
        max_segments: 1,
        ..LayoutLimits::default()
    };
    assert!(matches!(
        root.scan(limits),
        Err(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::Segments,
            ..
        })
    ));
}

#[test]
fn visited_entry_limit_accepts_the_boundary_and_rejects_the_next_entry() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("2024/01/01")).unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let exact = LayoutLimits {
        max_visited_entries: 3,
        ..LayoutLimits::default()
    };
    assert_eq!(root.scan(exact).unwrap().visited_entries, 3);

    let below = LayoutLimits {
        max_visited_entries: 2,
        ..LayoutLimits::default()
    };
    assert!(matches!(
        root.scan(below),
        Err(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::VisitedEntries,
            limit: 2,
        })
    ));
}

#[test]
fn entries_per_day_limit_accepts_the_boundary_and_rejects_the_next_entry() {
    let directory = tempfile::tempdir().unwrap();
    let day = directory.path().join("2024/02/29");
    std::fs::create_dir_all(&day).unwrap();
    let midnight = 1_709_164_800_000_000_i64;
    for offset in 0..2_i64 {
        std::fs::write(day.join(format!("{}.zms", midnight + offset)), b"ZMS").unwrap();
    }
    let root = DataRoot::open(directory.path()).unwrap();
    let exact = LayoutLimits {
        max_entries_per_day: 2,
        ..LayoutLimits::default()
    };
    assert_eq!(root.scan(exact).unwrap().segments.len(), 2);

    let below = LayoutLimits {
        max_entries_per_day: 1,
        ..LayoutLimits::default()
    };
    assert!(matches!(
        root.scan(below),
        Err(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::EntriesPerDay,
            limit: 1,
        })
    ));
}

#[test]
fn metadata_limit_accepts_the_boundary_and_rejects_the_next_byte() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("2024")).unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let exact_bytes = ENTRY_METADATA_BYTES + "2024".len();
    let exact = LayoutLimits {
        max_metadata_bytes: exact_bytes,
        ..LayoutLimits::default()
    };
    assert_eq!(root.scan(exact).unwrap().metadata_bytes, exact_bytes);

    let below = LayoutLimits {
        max_metadata_bytes: exact_bytes - 1,
        ..LayoutLimits::default()
    };
    assert!(matches!(
        root.scan(below),
        Err(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::MetadataBytes,
            limit,
        }) if limit == exact_bytes - 1
    ));
}

#[test]
fn zms_publication_rejects_a_replaced_temporary_name() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
    let address = address(1_709_164_801_000_000);
    let mut temporary = owner.create_zms_temp(address).unwrap();
    temporary.file_mut().write_all(b"expected ZMS").unwrap();
    let day = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    let temporary_name = temporary.temp_name.clone();
    std::fs::remove_file(day.join(&temporary_name)).unwrap();
    std::fs::write(day.join(&temporary_name), b"replacement").unwrap();

    assert!(matches!(
        temporary.publish(),
        Err(LayoutError::TemporaryChanged { .. })
    ));
    drop(temporary);
    assert!(!day.join(address.zms_name()).exists());
    assert_eq!(
        std::fs::read(day.join(&temporary_name)).unwrap(),
        b"replacement"
    );
}

#[test]
fn idx_publication_rejects_a_replaced_temporary_name() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let address = address(1_709_164_801_000_000);
    let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
    let mut zms = writer.create_zms_temp(address).unwrap();
    zms.file_mut().write_all(b"source ZMS").unwrap();
    zms.publish().unwrap();
    drop(zms);
    drop(writer);

    let owner = root.acquire_index(LayoutLimits::default()).unwrap();
    let mut temporary = owner.create_idx_temp(address).unwrap();
    temporary.file_mut().write_all(b"expected IDX").unwrap();
    let day = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    let temporary_name = temporary.temp_name.clone();
    std::fs::remove_file(day.join(&temporary_name)).unwrap();
    std::fs::write(day.join(&temporary_name), b"replacement").unwrap();

    assert!(matches!(
        temporary.publish(),
        Err(LayoutError::TemporaryChanged { .. })
    ));
    assert!(!day.join(address.idx_name()).exists());
    assert_eq!(
        std::fs::read(day.join(temporary_name)).unwrap(),
        b"replacement"
    );
}
