//! Publish.

use super::*;

#[test]
fn remove_finished_segment_unlinks_the_zms_and_reports_freed_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let older = address(1_709_164_801_000_000);
    let newer = address(1_709_164_802_000_000);
    let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
    for item in [older, newer] {
        let mut temp = writer.create_zms_temp(item).unwrap();
        temp.file_mut().write_all(b"ZMSBODY").unwrap();
        temp.publish().unwrap();
    }

    let removal = writer.remove_finished_segment(older).unwrap();
    assert_eq!(removal.zms_bytes, b"ZMSBODY".len() as u64);
    assert_eq!(removal.idx_bytes, None, "no sibling index was present");

    let snapshot = root.scan(LayoutLimits::default()).unwrap();
    assert_eq!(
        snapshot
            .segments
            .iter()
            .map(|segment| segment.address.id)
            .collect::<Vec<_>>(),
        vec![newer.id],
        "only the newer segment survives"
    );
}

#[test]
fn remove_finished_segment_frees_nothing_when_it_is_already_gone() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let older = address(1_709_164_801_000_000);
    let keeper = address(1_709_164_802_000_000);
    let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
    for item in [older, keeper] {
        let mut temp = writer.create_zms_temp(item).unwrap();
        temp.file_mut().write_all(b"ZMS").unwrap();
        temp.publish().unwrap();
    }
    writer.remove_finished_segment(older).unwrap();

    let second = writer.remove_finished_segment(older).unwrap();
    assert_eq!(second.total_bytes(), 0, "a repeated removal frees nothing");
}

#[test]
fn segment_removal_total_sums_the_zms_and_index() {
    assert_eq!(
        SegmentRemoval {
            zms_bytes: 100,
            idx_bytes: Some(30),
        }
        .total_bytes(),
        130
    );
    assert_eq!(
        SegmentRemoval {
            zms_bytes: 100,
            idx_bytes: None,
        }
        .total_bytes(),
        100
    );
}

#[test]
fn index_owner_prunes_empty_calendar_ancestors_bottom_up() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let address = address(1_709_164_801_000_000);
    let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
    let temp = writer.create_zms_temp(address).unwrap();
    temp.discard().unwrap();
    drop(writer);
    assert!(directory.path().join(address.day.year_component()).is_dir());

    let index = root.acquire_index(LayoutLimits::default()).unwrap();
    index.prune_empty_day(address.day).unwrap();

    assert!(!directory.path().join(address.day.year_component()).exists());
}

#[test]
fn index_does_not_prune_a_day_while_the_writer_owns_the_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let address = address(1_709_164_801_000_000);
    let writer = root.acquire_writer(LayoutLimits::default()).unwrap();
    let temp = writer.create_zms_temp(address).unwrap();
    temp.discard().unwrap();
    let index = root.acquire_index(LayoutLimits::default()).unwrap();

    index.prune_empty_day(address.day).unwrap();

    assert!(directory.path().join(address.day.year_component()).is_dir());
    drop(writer);
    index.prune_empty_day(address.day).unwrap();
    assert!(!directory.path().join(address.day.year_component()).exists());
}

#[test]
fn zms_publication_rejects_same_inode_rewrite_with_restored_mtime() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
    let address = address(1_709_164_801_000_000);
    let mut temporary = owner.create_zms_temp(address).unwrap();
    temporary.file_mut().write_all(b"expected ZMS").unwrap();
    let prepared = temporary.try_clone_file().unwrap();
    let prepared_identity = FileIdentity::from_file(&prepared).unwrap();
    let prepared_mtime = prepared.metadata().unwrap().modified().unwrap();
    drop(prepared);

    let day = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    let temporary_path = day.join(&temporary.temp_name);
    let rewritten_identity = rewrite_same_inode_with_restored_mtime(
        &temporary_path,
        prepared_identity,
        prepared_mtime,
        b"tampered ZMS",
    );
    assert_eq!(rewritten_identity.device, prepared_identity.device);
    assert_eq!(rewritten_identity.inode, prepared_identity.inode);
    assert_eq!(rewritten_identity.len, prepared_identity.len);
    assert_eq!(
        (
            rewritten_identity.mtime_seconds,
            rewritten_identity.mtime_nanoseconds
        ),
        (
            prepared_identity.mtime_seconds,
            prepared_identity.mtime_nanoseconds
        )
    );

    assert!(matches!(
        temporary.publish(),
        Err(LayoutError::TemporaryChanged { .. })
    ));
    assert!(!day.join(address.zms_name()).exists());
    assert_eq!(std::fs::read(temporary_path).unwrap(), b"tampered ZMS");
}

#[test]
fn prepared_idx_publishes_under_its_post_rename_identity() {
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
    drop(temporary.try_clone_file().unwrap());
    temporary.publish().unwrap();

    assert_eq!(
        std::fs::read(root.diagnostic_file_path(address, FileKind::Idx)).unwrap(),
        b"expected IDX"
    );
}

#[test]
fn idx_publication_rejects_same_inode_rewrite_with_restored_mtime() {
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
    let prepared = temporary.try_clone_file().unwrap();
    let prepared_identity = FileIdentity::from_file(&prepared).unwrap();
    let prepared_mtime = prepared.metadata().unwrap().modified().unwrap();
    drop(prepared);

    let day = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    let temporary_path = day.join(&temporary.temp_name);
    let rewritten_identity = rewrite_same_inode_with_restored_mtime(
        &temporary_path,
        prepared_identity,
        prepared_mtime,
        b"tampered IDX",
    );
    assert_eq!(rewritten_identity.device, prepared_identity.device);
    assert_eq!(rewritten_identity.inode, prepared_identity.inode);
    assert_eq!(rewritten_identity.len, prepared_identity.len);
    assert_eq!(
        (
            rewritten_identity.mtime_seconds,
            rewritten_identity.mtime_nanoseconds
        ),
        (
            prepared_identity.mtime_seconds,
            prepared_identity.mtime_nanoseconds
        )
    );

    assert!(matches!(
        temporary.publish(),
        Err(LayoutError::TemporaryChanged { .. })
    ));
    assert!(!day.join(address.idx_name()).exists());
    assert_eq!(
        std::fs::read(day.join(address.zms_name())).unwrap(),
        b"source ZMS"
    );
}
