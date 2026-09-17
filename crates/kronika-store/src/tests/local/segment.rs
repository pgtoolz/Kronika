//! Listing a data root and excluding what cannot be read.

use super::*;

#[test]
fn catalog_summary_transient_bytes_are_admitted_before_allocation() {
    let buf = part(1000);
    let error = read_validated_zms_summary(&buf.as_slice(), 1, 1).unwrap_err();
    assert!(matches!(
        error,
        StoreError::Layout(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::MetadataBytes,
            limit: 1,
        })
    ));
}

#[test]
fn scan_lists_finished_and_active_with_cheap_catalog() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1000));
    write_journal(dir.path(), 2_000, &[part(2000), part(3000)]);
    let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(scan.finished.len(), 1, "one finished segment");
    assert_eq!(
        scan.finished[0].summary.min_ts, 1000,
        "finished summary min_ts"
    );
    assert_eq!(scan.active.len(), 2, "two active parts");
    assert_eq!(
        scan.active[1].catalog.min_ts, 3000,
        "second active part min_ts"
    );
    assert_eq!(
        scan.active[1].catalog_digest,
        CatalogDigest::from_catalog(&scan.active[1].catalog),
        "the validated catalog digest is retained with the active part"
    );
    assert!(scan.warnings.is_empty(), "no warnings for clean data");
}

#[test]
fn cached_scan_reuses_summary_without_reading_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1000));
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();

    reset_catalog_summary_reads();
    let second = local
        .complete_scan_cached(local.scan_journal().unwrap(), first.finished.as_slice())
        .unwrap();

    assert_eq!(
        catalog_summary_reads(),
        0,
        "an identity-equal ZMS must not have its catalog reread"
    );
    assert!(Arc::ptr_eq(
        &first.finished[0].summary,
        &second.finished[0].summary
    ));
    let cloned = second.clone();
    assert!(
        Arc::ptr_eq(&second.finished, &cloned.finished),
        "snapshot clone must share the finished collection"
    );
}

#[test]
fn changed_identity_does_not_reuse_a_corrupt_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = segment_path(dir.path(), 1_000);
    let valid = part(1000);
    fs::write(&path, &valid).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();

    let mut corrupt = valid;
    let corrupt_at = catalog_offset(&corrupt);
    corrupt[corrupt_at] ^= 0xff;
    fs::write(path, corrupt).unwrap();
    reset_catalog_summary_reads();
    let scan = local
        .complete_scan_cached(local.scan_journal().unwrap(), first.finished.as_slice())
        .unwrap();

    assert!(scan.finished.is_empty());
    assert_eq!(
        invalid_warning(&scan, 1_000).reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::Catalog)
    );
    assert_eq!(catalog_summary_reads(), 1);
}

#[test]
fn incremental_scan_counts_cached_parts_toward_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let first_bytes = journal(2_000, &[part(2000)]);
    let complete_bytes = journal(2_000, &[part(2000), part(3000)]);
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local
        .scan_journal_reader_bounded_from(
            &first_bytes,
            JOURNAL_HEADER_LEN as u64,
            Arc::new(Vec::new()),
            &journal_path,
            1,
        )
        .unwrap();
    let first_valid_len = first.valid_len;

    let error = local
        .scan_journal_reader_bounded_from(
            &complete_bytes,
            first_valid_len,
            first.active,
            &journal_path,
            1,
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("allowed 1 active parts"));
}

#[test]
fn corrupt_finished_is_excluded_while_valid_segments_continue() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1000));
    write_segment(dir.path(), 2_000, b"not a zms");
    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local.scan().unwrap();

    assert_eq!(scan.finished.len(), 1);
    assert_eq!(scan.finished[0].address.id.get(), 1_000);
    assert_eq!(
        invalid_warning(&scan, 2_000).reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::TailIndex)
    );
    let opened = local.open_finished(&scan.finished[0]).unwrap();
    assert_eq!(read_catalog(&opened).unwrap().min_ts, 1_000);
}

#[test]
fn full_zms_validation_classifies_catalog_geometry_and_section_crc() {
    let cases = [
        (
            2_000,
            {
                let mut bytes = part(2_000);
                let corrupt_at = catalog_offset(&bytes);
                bytes[corrupt_at] ^= 0xff;
                bytes
            },
            InvalidZmsReason::Catalog,
        ),
        (
            3_000,
            {
                let mut bytes = part(3_000);
                let entry_offset_at = catalog_offset(&bytes) + 8;
                bytes[entry_offset_at..entry_offset_at + 8].copy_from_slice(&5_u64.to_le_bytes());
                repatch_catalog_crc(&mut bytes);
                bytes
            },
            InvalidZmsReason::CanonicalLayout,
        ),
        (
            4_000,
            {
                let mut bytes = part_with_body(4_000, b"section-secret");
                bytes[MAGIC.len()] ^= 0xff;
                bytes
            },
            InvalidZmsReason::SectionChecksum,
        ),
    ];

    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1_000));
    for (raw_id, bytes, _reason) in &cases {
        write_segment(dir.path(), *raw_id, bytes);
    }

    let first = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    let restarted = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(first.finished.len(), 1);
    assert_eq!(first.finished[0].address.id.get(), 1_000);
    assert_eq!(first.warnings, restarted.warnings);
    for (raw_id, _bytes, reason) in cases {
        let warning = invalid_warning(&first, raw_id);
        assert_eq!(warning.reason, StoreWarningReason::InvalidZms(reason));
        assert!(warning.identity.is_some());
        assert!(warning.failure.is_none());
        let diagnostic = format!("{warning:?}");
        assert!(!diagnostic.contains("section-secret"));
        assert!(!diagnostic.contains(&dir.path().display().to_string()));
    }
}

#[test]
fn catalog_scan_defers_body_checksums_until_selected_validation() {
    let dir = tempfile::tempdir().unwrap();
    let mut damaged_body = part_with_body(2_000, b"body whose CRC will no longer match");
    damaged_body[MAGIC.len()] ^= 0xff;
    write_segment(dir.path(), 2_000, damaged_body);
    let local = LocalDir::open(dir.path()).unwrap();

    let mut discovered = local.scan_catalogs().unwrap();
    assert_eq!(discovered.finished.len(), 1);
    assert!(discovered.warnings.is_empty());

    let selected = discovered.finished[0].clone();
    assert!(!local.validate_finished(&mut discovered, &selected).unwrap());
    let warning = discovered
        .warnings
        .last()
        .expect("selected body is invalid");
    assert_eq!(
        warning.reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::SectionChecksum)
    );

    let strict = local.scan().unwrap();
    assert!(strict.finished.is_empty());
    assert_eq!(
        invalid_warning(&strict, 2_000).reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::SectionChecksum)
    );
}

#[test]
fn selected_validation_counts_the_retained_catalog_scan_against_the_budget() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 2_000, part_with_body(2_000, b"selected body"));
    let mut local = LocalDir::open(dir.path()).unwrap();
    let mut discovered = local.scan_catalogs().unwrap();
    let selected = discovered.finished[0].clone();

    local.limits.max_metadata_bytes = discovered.metadata_bytes;
    let error = local
        .validate_finished(&mut discovered, &selected)
        .unwrap_err();
    let store_error = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<StoreError>());
    assert!(matches!(
        store_error,
        Some(StoreError::Layout(LayoutError::TraversalLimitExceeded {
            kind: LimitKind::MetadataBytes,
            ..
        }))
    ));
}

#[test]
fn identity_change_during_invalid_validation_is_interrupted_not_excluded() {
    let dir = tempfile::tempdir().unwrap();
    let path = segment_path(dir.path(), 2_000);
    fs::write(&path, b"not a zms").unwrap();
    let address = SegmentAddress::new(SegmentId::new(2_000).unwrap()).unwrap();
    let file = File::open(&path).unwrap();
    let expected = FileIdentity::from_file(&file).unwrap();
    let validation =
        read_validated_zms_summary(&file, 0, LayoutLimits::default().max_metadata_bytes);
    assert!(validation.is_err());

    fs::write(&path, part(2_000)).unwrap();
    let error = classify_zms_validation(&file, expected, address, validation).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}

#[test]
fn unreadable_zms_degrades_locally_with_typed_io_details() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1_000));
    let unreadable = segment_path(dir.path(), 2_000);
    fs::write(&unreadable, part(2_000)).unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();

    let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(scan.finished.len(), 1);
    let warning = invalid_warning(&scan, 2_000);
    assert_eq!(
        warning.reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::Io)
    );
    let failure = warning.failure.expect("typed I/O failure");
    assert_eq!(failure.operation, StoreIoOperation::Open);
    assert_eq!(failure.error_kind, io::ErrorKind::PermissionDenied);
}

#[test]
fn foreign_file_and_directory_do_not_hide_valid_segments_or_leak_names() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1_000));
    fs::create_dir(dir.path().join("lost+found")).unwrap();
    fs::write(dir.path().join(".nfs-private-name"), b"foreign-secret").unwrap();

    let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(scan.finished.len(), 1);
    let foreign = scan
        .warnings
        .iter()
        .filter(|warning| matches!(warning.affected, StoreObject::Foreign(_)))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(foreign.len(), 2);
    assert!(foreign.iter().all(|warning| {
        matches!(warning.reason, StoreWarningReason::ForeignEntry(_))
            && warning.identity.is_some()
            && warning.failure.is_none()
    }));
    let diagnostic = format!("{foreign:?}");
    assert!(!diagnostic.contains("lost+found"));
    assert!(!diagnostic.contains(".nfs-private-name"));
    assert!(!diagnostic.contains("foreign-secret"));
}
