//! Ownership.

use super::*;

#[test]
fn flat_segment_is_excluded_without_reading_it() {
    for name in ["1000.zms", "1000.idx"] {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(name), b"not a container").unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert!(snapshot.segments.is_empty());
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::UnsupportedFlatArtifact
        );
    }
}

#[test]
fn symlinked_calendar_component_is_excluded_without_following() {
    let directory = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    symlink(target.path(), directory.path().join("2024")).unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let snapshot = root.scan(LayoutLimits::default()).unwrap();
    assert!(snapshot.days.is_empty());
    assert_eq!(snapshot.foreign_entries.len(), 1);
    assert_eq!(
        snapshot.foreign_entries[0].diagnostic().reason,
        ForeignEntryReason::SymbolicLink
    );
}

#[test]
fn symlinks_are_excluded_at_month_day_and_leaf_levels() {
    for level in ["month", "day", "leaf"] {
        let directory = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        match level {
            "month" => {
                std::fs::create_dir(directory.path().join("2024")).unwrap();
                symlink(target.path(), directory.path().join("2024/02")).unwrap();
            }
            "day" => {
                std::fs::create_dir_all(directory.path().join("2024/02")).unwrap();
                symlink(target.path(), directory.path().join("2024/02/29")).unwrap();
            }
            "leaf" => {
                let day = directory.path().join("2024/02/29");
                std::fs::create_dir_all(&day).unwrap();
                let target_file = target.path().join("segment");
                std::fs::write(&target_file, b"ZMS").unwrap();
                symlink(&target_file, day.join("1709164800000000.zms")).unwrap();
            }
            _ => unreachable!(),
        }
        let root = DataRoot::open(directory.path()).unwrap();
        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert!(
            snapshot.segments.is_empty(),
            "{level} symlink must not become a segment"
        );
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::SymbolicLink
        );
    }
}

#[test]
fn noncanonical_segment_names_are_excluded() {
    for name in ["+1.zms", "01.zms", "-0.zms"] {
        let directory = tempfile::tempdir().unwrap();
        let day = directory.path().join("1970/01/01");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join(name), b"ZMS").unwrap();
        let root = DataRoot::open(directory.path()).unwrap();
        let snapshot = root.scan(LayoutLimits::default()).unwrap();
        assert!(snapshot.segments.is_empty());
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::UnsupportedName,
            "{name} must not alias a canonical SegmentId"
        );
    }
}

#[test]
fn canonical_integer_components_accept_only_their_decimal_spelling() {
    for value in ["0", "1", "18446744073709551615"] {
        assert!(names::parse_canonical_u64(value).is_some(), "{value}");
    }
    for value in ["-9223372036854775808", "-1", "0", "9223372036854775807"] {
        assert!(names::parse_canonical_i64(value).is_some(), "{value}");
    }
    for value in ["", "+1", "-0", "00", "01", "-01", "1a", " 1", "1 "] {
        assert!(names::parse_canonical_i64(value).is_none(), "{value}");
    }
    for value in ["", "+1", "-1", "00", "01", "1a", " 1", "1 "] {
        assert!(names::parse_canonical_u64(value).is_none(), "{value}");
    }
    assert!(names::parse_canonical_i64("9223372036854775808").is_none());
    assert!(names::parse_canonical_u64("18446744073709551616").is_none());
}

#[test]
fn misbucketed_segment_is_excluded() {
    let directory = tempfile::tempdir().unwrap();
    let day = directory.path().join("2024/02/28");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("1709164800000000.zms"), b"ZMS").unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let snapshot = root.scan(LayoutLimits::default()).unwrap();
    assert!(snapshot.segments.is_empty());
    assert_eq!(snapshot.foreign_entries.len(), 1);
    assert_eq!(
        snapshot.foreign_entries[0].diagnostic().reason,
        ForeignEntryReason::MisbucketedSegment
    );
}

#[test]
fn one_writer_owner_is_enforced() {
    let directory = tempfile::tempdir().unwrap();
    let first_root = DataRoot::open(directory.path()).unwrap();
    let second_root = DataRoot::open(directory.path()).unwrap();
    let _first = first_root.acquire_writer(LayoutLimits::default()).unwrap();
    assert!(matches!(
        second_root.acquire_writer(LayoutLimits::default()),
        Err(LayoutError::OwnerContended {
            owner: OwnerKind::Writer
        })
    ));
}

#[test]
fn cloned_writer_lease_keeps_exclusive_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let first_root = DataRoot::open(directory.path()).unwrap();
    let second_root = DataRoot::open(directory.path()).unwrap();
    let owner = first_root.acquire_writer(LayoutLimits::default()).unwrap();
    let lease = owner.try_clone_lease().unwrap();
    drop(owner);

    assert!(matches!(
        second_root.acquire_writer(LayoutLimits::default()),
        Err(LayoutError::OwnerContended {
            owner: OwnerKind::Writer
        })
    ));

    drop(lease);
    second_root.acquire_writer(LayoutLimits::default()).unwrap();
}

#[test]
fn direct_open_rejects_a_fifo_without_blocking() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let address = address(1_709_164_801_000_000);
    let day = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    std::fs::create_dir_all(&day).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(day.join(address.zms_name()))
            .status()
            .unwrap()
            .success()
    );

    assert!(matches!(
        root.open_zms(address),
        Err(LayoutError::UnexpectedLeafEntryType { .. })
    ));
}

#[test]
fn one_day_directory_opens_only_segments_from_that_day() {
    let directory = tempfile::tempdir().unwrap();
    let first = address(1_709_164_801_000_000);
    let second = address(1_709_251_201_000_000);
    for address in [first, second] {
        let day = directory
            .path()
            .join(address.day.year_component())
            .join(address.day.month_component())
            .join(address.day.day_component());
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join(address.zms_name()), b"ZMS").unwrap();
    }
    let root = DataRoot::open(directory.path()).unwrap();
    let day = root.day_directory(first.day).unwrap();
    assert_eq!(day.open_zms(first).unwrap().metadata().unwrap().len(), 3);
    assert!(matches!(
        day.open_zms(second),
        Err(LayoutError::MisbucketedSegment { .. })
    ));
}
