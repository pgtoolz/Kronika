//! The metadata a scan is allowed to retain.

use super::*;

#[test]
fn active_part_budget_includes_retained_metadata_and_transient_body() {
    assert!(ensure_active_part_budget(100, 20, 30, 150).is_ok());
    let error = ensure_active_part_budget(100, 20, 30, 149).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn active_vector_capacity_and_reallocation_peak_are_budgeted() {
    let mut active = Arc::new(Vec::<ActivePart>::with_capacity(1));
    let retained = active_metadata_bytes(&active, active.capacity()).unwrap();
    assert_eq!(
        retained,
        size_of::<ActivePart>() + ACTIVE_ARC_ALLOCATION_BYTES
    );

    let replacement = 2 * size_of::<ActivePart>();
    let limit = retained + replacement;
    let error = reserve_active_slots(&mut active, 2, retained, 0, 0, limit - 1).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(active.capacity(), 1, "rejection happens before allocation");

    reserve_active_slots(&mut active, 2, retained, 0, 0, limit).unwrap();
    assert!(active.capacity() >= 2);
    assert_eq!(
        active_metadata_bytes(&active, active.capacity()).unwrap(),
        active.capacity() * size_of::<ActivePart>() + ACTIVE_ARC_ALLOCATION_BYTES
    );
}

#[test]
fn shared_active_clone_is_admitted_before_copy_on_write() {
    let catalog = read_catalog(&part(1000).as_slice()).expect("catalog");
    let catalog_digest = CatalogDigest::from_catalog(&catalog);
    let mut active = Arc::new(vec![ActivePart {
        segment_id: SegmentId::new(1_000).unwrap(),
        part: PartRef {
            offset: JOURNAL_HEADER_LEN + FRAME_HEADER_LEN,
            len: 1,
        },
        catalog,
        catalog_digest,
    }]);
    let retained = active_metadata_bytes(&active, active.capacity()).unwrap();
    let previous = Arc::clone(&active);
    let clone_peak = retained.checked_mul(2).expect("test metadata fits");

    let error =
        reserve_active_slots(&mut active, 1, retained, 0, retained, clone_peak - 1).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        Arc::ptr_eq(&active, &previous),
        "the shared baseline must remain untouched when its clone is not admitted"
    );
}

#[test]
fn million_part_clone_lower_bound_exceeds_the_default_metadata_budget() {
    let one_baseline = MAX_JOURNAL_PARTS
        .checked_mul(size_of::<ActivePart>())
        .and_then(|bytes| bytes.checked_add(ACTIVE_ARC_ALLOCATION_BYTES))
        .expect("v1 active baseline size fits usize");
    let clone_peak = one_baseline
        .checked_mul(2)
        .expect("v1 active clone peak fits usize");
    let limit = LayoutLimits::default().max_metadata_bytes;

    assert!(
        one_baseline <= limit,
        "one minimal maximum-count baseline remains representable"
    );
    assert!(
        clone_peak > limit,
        "a live clone makes copy-on-write inadmissible before catalog-entry allocations"
    );
}

#[test]
fn same_inode_metadata_change_invalidates_cached_and_opened_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = segment_path(dir.path(), 1_000);
    fs::write(&path, part(1000)).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();
    let pinned = first.finished[0].clone();
    let opened = local.open_finished(&pinned).unwrap();

    fs::write(&path, part_with_body(1000, b"changed")).unwrap();
    let current_identity = FileIdentity::from_file(&File::open(&path).unwrap()).unwrap();
    assert_eq!(pinned.identity.device, current_identity.device);
    assert_eq!(pinned.identity.inode, current_identity.inode);
    assert_ne!(pinned.identity, current_identity);
    assert_eq!(
        local
            .validate_finished_file(&opened, &pinned)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(
        local.open_finished(&pinned).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );

    reset_catalog_summary_reads();
    let second = local
        .complete_scan_cached(local.scan_journal().unwrap(), first.finished.as_slice())
        .unwrap();
    assert_eq!(catalog_summary_reads(), 1);
    assert!(!Arc::ptr_eq(
        &first.finished[0].summary,
        &second.finished[0].summary
    ));
    assert_eq!(second.finished[0].identity, current_identity);
}

#[test]
fn five_year_fixed_summary_scan_fits_the_metadata_budget() {
    const SEGMENTS: usize = 5 * 35_040;
    const PERMANENT_FILES: usize = 5 * 70_080;
    const CALENDAR_DIRECTORIES: usize = 5 * (1 + 12 + 365);
    const ENTRY_ACCOUNTING: usize = 128;
    const MAX_FILE_NAME_BYTES: usize = 24;
    const LIMIT: usize = 128 * 1024 * 1024;

    let layout_metadata = PERMANENT_FILES * (ENTRY_ACCOUNTING + MAX_FILE_NAME_BYTES)
        + CALENDAR_DIRECTORIES * (ENTRY_ACCOUNTING + 4)
        + SEGMENTS * size_of::<SegmentArtifacts>();
    let cold = accounted_scan_metadata_bytes(layout_metadata, 0, 0, SEGMENTS, SEGMENTS).unwrap();
    let unchanged_refresh =
        accounted_scan_metadata_bytes(layout_metadata, 0, SEGMENTS, SEGMENTS, 0).unwrap();
    let replaced_refresh =
        accounted_scan_metadata_bytes(layout_metadata, 0, SEGMENTS, SEGMENTS, SEGMENTS).unwrap();

    assert!(cold <= LIMIT, "cold five-year scan accounts {cold} bytes");
    assert!(
        unchanged_refresh <= LIMIT,
        "unchanged five-year refresh accounts {unchanged_refresh} bytes"
    );
    assert!(
        replaced_refresh > LIMIT,
        "a full same-name replacement must fail before duplicating every summary"
    );
}
