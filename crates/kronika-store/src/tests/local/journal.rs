//! Reading `active.wal` across appends, resets and damage.

use super::*;

#[test]
fn read_active_part_rejects_oversized_ref_before_allocation() {
    let dir = tempfile::tempdir().unwrap();
    write_empty_journal(dir.path());
    let catalog = read_catalog(&part(1000).as_slice()).expect("catalog");
    let catalog_digest = CatalogDigest::from_catalog(&catalog);
    let oversized_len = usize::try_from(MAX_PART_LEN).expect("part cap fits usize") + 1;
    let active = ActivePart {
        segment_id: SegmentId::new(1_000).unwrap(),
        part: PartRef {
            offset: JOURNAL_HEADER_LEN + FRAME_HEADER_LEN,
            len: oversized_len,
        },
        catalog,
        catalog_digest,
    };

    let err = LocalDir::open(dir.path())
        .unwrap()
        .read_active_part(&active)
        .unwrap_err();

    assert!(
        matches!(
            err,
            StoreError::ActivePartTooLarge { len, max }
                if len == oversized_len && max == MAX_PART_LEN
        ),
        "oversized active part must be rejected before allocation"
    );
}

#[test]
fn read_active_part_rejects_the_same_offset_and_catalog_in_a_new_segment() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = part(1000);
    write_journal(dir.path(), 1_000, std::slice::from_ref(&bytes));
    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local.scan().unwrap();
    let active = scan.active[0].clone();

    write_journal(dir.path(), 2_000, std::slice::from_ref(&bytes));
    let error = local.read_active_part(&active).unwrap_err();
    assert!(matches!(error, StoreError::Io(source) if source.kind() == io::ErrorKind::NotFound));
}

#[test]
fn read_active_part_ignores_an_uncommitted_append_tail() {
    let dir = tempfile::tempdir().unwrap();
    let first = part(1000);
    let later = part(2000);
    write_journal(dir.path(), 1_000, std::slice::from_ref(&first));
    let journal_path = dir.path().join("active.wal");
    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local.scan().unwrap();
    let active = scan.active[0].clone();

    let mut with_uncommitted_tail = fs::read(&journal_path).unwrap();
    with_uncommitted_tail.extend_from_slice(&frame(&later));
    fs::write(journal_path, with_uncommitted_tail).unwrap();

    assert_eq!(local.read_active_part(&active).unwrap(), first);
}

#[test]
fn active_snapshot_reads_only_its_prefix_after_a_later_append() {
    let dir = tempfile::tempdir().unwrap();
    let first = part(1000);
    let later = part(2000);
    write_journal(dir.path(), 1_000, std::slice::from_ref(&first));
    let journal_path = dir.path().join("active.wal");
    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local.scan_catalogs().unwrap();
    let snapshot = local
        .open_active_snapshot(&scan)
        .unwrap()
        .expect("active prefix");

    append_journal_part(&journal_path, 1_000, &later);

    assert_eq!(
        snapshot.read_part_range(0, 0, first.len() as u64).unwrap(),
        first
    );
    assert!(matches!(
        snapshot.read_part_range(0, 0, first.len() as u64 + 1),
        Err(StoreError::OutOfBounds)
    ));
    assert_eq!(local.scan_catalogs().unwrap().active.len(), 2);
}

#[test]
fn active_snapshot_rejects_a_new_journal_generation() {
    let dir = tempfile::tempdir().unwrap();
    let first = part(1000);
    write_journal(dir.path(), 1_000, std::slice::from_ref(&first));
    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local.scan_catalogs().unwrap();
    let snapshot = local
        .open_active_snapshot(&scan)
        .unwrap()
        .expect("active prefix");

    write_journal(dir.path(), 2_000, &[part(2000)]);

    assert!(matches!(
        snapshot.read_part_range(0, 0, first.len() as u64),
        Err(StoreError::Io(source)) if source.kind() == io::ErrorKind::NotFound
    ));
}

#[test]
fn journal_above_the_v1_physical_limit_is_rejected_before_scanning() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let file = File::create(&journal_path).unwrap();
    file.set_len(MAX_JOURNAL_LEN as u64 + 1).unwrap();

    let error = LocalDir::open(dir.path())
        .unwrap()
        .scan_journal()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("version-1 limit"));
    assert_eq!(
        fs::metadata(journal_path).unwrap().len(),
        MAX_JOURNAL_LEN as u64 + 1
    );
}

#[test]
fn active_part_count_limit_is_stable_invalid_data() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let bytes = journal(2_000, &[part(2000), part(3000)]);
    let local = LocalDir::open(dir.path()).unwrap();

    let error = local
        .scan_journal_reader_bounded_from(
            &bytes,
            JOURNAL_HEADER_LEN as u64,
            Arc::new(Vec::new()),
            &journal_path,
            1,
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("allowed 1 active parts"));
}

#[test]
fn zero_length_active_journal_reads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1000));
    fs::write(dir.path().join("active.wal"), []).unwrap();
    let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(scan.finished.len(), 1);
    assert!(scan.active.is_empty());
}

#[test]
fn torn_active_journal_degrades_to_finished_only_scan() {
    let dir = tempfile::tempdir().unwrap();
    write_segment(dir.path(), 1_000, part(1000));
    let unfinished_part = part(2000);
    let body = FrameHeader {
        part_len: u64::try_from(unfinished_part.len()).expect("part length fits u64"),
    }
    .encode();
    let mut bytes = JournalHeader {
        state: JournalState::Active { segment_id: 2_000 },
        body_len: body.len() as u64,
    }
    .encode()
    .to_vec();
    bytes.extend_from_slice(&body);
    fs::write(dir.path().join("active.wal"), bytes).unwrap();
    let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
    assert_eq!(scan.finished.len(), 1);
    assert!(scan.active.is_empty());
    assert!(scan.warnings.iter().any(|warning| {
        warning.affected == StoreObject::ActiveJournal
            && matches!(
                warning.reason,
                StoreWarningReason::ActiveJournal(
                    ActiveJournalWarningReason::Corrupt | ActiveJournalWarningReason::Io
                )
            )
    }));
}

#[test]
fn committed_reset_phases_are_logically_empty_after_validating_the_old_body() {
    for phase in [
        CommittedHeaderPhase::Previous,
        CommittedHeaderPhase::Empty,
        CommittedHeaderPhase::Torn,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let bytes = committed_reset_journal(2_000, &[part(2000)], phase);
        fs::write(dir.path().join("active.wal"), &bytes).unwrap();

        let scan = LocalDir::open(dir.path()).unwrap().scan().unwrap();
        assert!(scan.active.is_empty());
        assert_eq!(
            scan.valid_len,
            bytes.len() as u64,
            "the complete committed state is a valid logical reset boundary"
        );
    }
}

#[test]
fn committed_reset_marker_does_not_hide_corrupt_old_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = committed_reset_journal(2_000, &[part(2000)], CommittedHeaderPhase::Previous);
    bytes[JOURNAL_HEADER_LEN + FRAME_HEADER_LEN] ^= 0xff;
    fs::write(dir.path().join("active.wal"), bytes).unwrap();

    let error = LocalDir::open(dir.path())
        .unwrap()
        .scan_journal()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(is_active_journal_scan_error(&error));
}

#[test]
fn scan_error_origin_distinguishes_active_journal_from_finished_zms() {
    let active_dir = tempfile::tempdir().unwrap();
    fs::write(
        active_dir.path().join("active.wal"),
        b"stable malformed journal",
    )
    .unwrap();
    let active_error = LocalDir::open(active_dir.path())
        .unwrap()
        .scan_journal()
        .unwrap_err();
    assert!(is_active_journal_scan_error(&active_error));

    let finished_dir = tempfile::tempdir().unwrap();
    write_segment(finished_dir.path(), 2_000, b"stable malformed ZMS");
    let scan = LocalDir::open(finished_dir.path()).unwrap().scan().unwrap();
    assert!(scan.finished.is_empty());
    assert_eq!(
        invalid_warning(&scan, 2_000).reason,
        StoreWarningReason::InvalidZms(InvalidZmsReason::TailIndex)
    );
}

#[test]
fn scan_from_unchanged_size_keeps_prev_and_reports_same_valid_len() {
    let dir = tempfile::tempdir().unwrap();
    let journal = journal(1_000, &[part(1000)]);
    fs::write(dir.path().join("active.wal"), &journal).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();

    let first = local.scan().unwrap();
    assert_eq!(first.active.len(), 1);
    assert_eq!(first.valid_len, journal.len() as u64);

    let prev_active = Arc::clone(&first.active);
    let again = local
        .scan_from(first.valid_len, Arc::clone(&prev_active))
        .unwrap();
    assert_eq!(again.active.len(), 1, "unchanged journal keeps the part");
    assert_eq!(again.active[0].catalog.min_ts, 1000);
    assert_eq!(again.valid_len, journal.len() as u64);
    assert!(
        Arc::ptr_eq(&again.active, &prev_active),
        "an unchanged scan must retain the validated active allocation"
    );
}

#[test]
fn scan_from_appends_only_the_new_tail_part() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let journal = journal(1_000, &[part(1000)]);
    fs::write(&journal_path, &journal).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();

    let first = local.scan().unwrap();
    let first_valid = first.valid_len;
    let first_offset = first.active[0].part.offset;
    let previous = Arc::clone(&first.active);

    // Append a second frame.
    let buf = append_journal_part(&journal_path, 1_000, &part(3000));

    let scan = local
        .scan_from(first_valid, Arc::clone(&first.active))
        .unwrap();
    assert_eq!(scan.active.len(), 2, "prev part kept, new tail appended");
    assert!(
        !Arc::ptr_eq(&scan.active, &previous),
        "appending must leave the previous snapshot allocation immutable"
    );
    assert_eq!(
        previous.len(),
        1,
        "the previous snapshot keeps its exact view"
    );
    assert_eq!(
        scan.active[0].part.offset, first_offset,
        "the first part keeps its original offset"
    );
    assert_eq!(scan.active[0].catalog.min_ts, 1000);
    assert_eq!(scan.active[1].catalog.min_ts, 3000);
    assert_eq!(scan.valid_len, buf.len() as u64);
}

#[test]
fn scan_from_size_shrink_resets_and_rescans_from_zero() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    // Two frames make the initial journal larger than one replacement frame.
    let two = journal(1_000, &[part(1000), part(2000)]);
    fs::write(&journal_path, &two).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();

    let first = local.scan().unwrap();
    assert_eq!(first.active.len(), 2);
    let stale_valid = first.valid_len;

    // Truncate-in-place then write a smaller, different journal.
    let replacement = journal(5_000, &[part(5000)]);
    assert!(
        (replacement.len() as u64) < stale_valid,
        "replacement is smaller"
    );
    fs::write(&journal_path, &replacement).unwrap();

    let scan = local.scan_from(stale_valid, first.active).unwrap();
    assert_eq!(scan.active.len(), 1, "reset yields exactly the new journal");
    assert_eq!(
        scan.active[0].catalog.min_ts, 5000,
        "stale parts are dropped, only the new part surfaces"
    );
    assert_eq!(scan.valid_len, replacement.len() as u64);
}

#[test]
fn scan_from_missing_journal_resets_to_empty() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    fs::write(&journal_path, journal(1_000, &[part(1000)])).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();

    fs::remove_file(&journal_path).unwrap();

    let scan = local.scan_from(first.valid_len, first.active).unwrap();
    assert!(
        scan.active.is_empty(),
        "removed journal empties the live set"
    );
    assert_eq!(scan.valid_len, 0, "valid_len resets to zero");
}

#[test]
fn scan_from_torn_tail_returns_a_typed_empty_live_generation() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let journal = journal(1_000, &[part(1000)]);
    fs::write(&journal_path, &journal).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();
    let first_valid = first.valid_len;

    // Append a header for a body that is not fully written yet.
    let next = part(3000);
    let mut buf = fs::read(&journal_path).unwrap();
    let full = frame(&next);
    buf.extend_from_slice(&full[..full.len() - 3]); // truncated tail frame
    let body_len = buf.len() - JOURNAL_HEADER_LEN;
    buf[..JOURNAL_HEADER_LEN].copy_from_slice(
        &JournalHeader {
            state: JournalState::Active { segment_id: 1_000 },
            body_len: body_len as u64,
        }
        .encode(),
    );
    fs::write(&journal_path, &buf).unwrap();

    let scan = local.scan_from(first_valid, first.active).unwrap();
    assert!(scan.active.is_empty());
    assert_eq!(scan.valid_len, 0);
    assert!(matches!(
        scan.warnings.as_slice(),
        [StoreWarning {
            affected: StoreObject::ActiveJournal,
            reason: StoreWarningReason::ActiveJournal(ActiveJournalWarningReason::Corrupt),
            ..
        }]
    ));
}

#[test]
fn scan_from_discovers_new_finished_segment() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    fs::write(&journal_path, journal(1_000, &[part(1000)])).unwrap();
    let local = LocalDir::open(dir.path()).unwrap();
    let first = local.scan().unwrap();
    assert_eq!(first.finished.len(), 0);

    write_segment(dir.path(), 500, part(500));

    let scan = local.scan_from(first.valid_len, first.active).unwrap();
    assert_eq!(scan.finished.len(), 1, "new finished .zms is discovered");
    assert_eq!(scan.finished[0].summary.min_ts, 500);
    assert_eq!(scan.active.len(), 1, "active part is preserved");
}

// A concurrent shrink is reported as transient interruption. It must not
// turn a partial prefix into an authoritative live set.
#[test]
fn scan_reports_journal_truncated_mid_frame_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let p = part(2000);
    let frame_header = FrameHeader {
        part_len: p.len() as u64,
    }
    .encode();
    let body_len = (FRAME_HEADER_LEN + p.len()) as u64;
    let mut data = JournalHeader {
        state: JournalState::Active { segment_id: 2_000 },
        body_len,
    }
    .encode()
    .to_vec();
    data.extend_from_slice(&frame_header);
    let reported_len = JOURNAL_HEADER_LEN as u64 + body_len;
    let mock = TruncatedAfterHeader { data, reported_len };

    let local = LocalDir::open(dir.path()).unwrap();
    assert_eq!(
        local
            .scan_journal_reader_from(
                &mock,
                JOURNAL_HEADER_LEN as u64,
                Arc::new(Vec::new()),
                &journal_path,
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn scan_reports_journal_truncated_at_second_frame_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");

    let p1 = part(2000);
    let header1 = FrameHeader {
        part_len: p1.len() as u64,
    }
    .encode();
    let p2_fake_len = 512_u64; // claimed body size, not present in data
    let header2 = FrameHeader {
        part_len: p2_fake_len,
    }
    .encode();

    let body_len = (FRAME_HEADER_LEN + p1.len()) as u64 + FRAME_HEADER_LEN as u64 + p2_fake_len;
    let mut data = JournalHeader {
        state: JournalState::Active { segment_id: 2_000 },
        body_len,
    }
    .encode()
    .to_vec();
    data.extend_from_slice(&header1);
    data.extend_from_slice(&p1);
    data.extend_from_slice(&header2);
    let reported_len = JOURNAL_HEADER_LEN as u64 + body_len;

    let mock = TruncatedAfterHeader { data, reported_len };

    let local = LocalDir::open(dir.path()).unwrap();
    assert_eq!(
        local
            .scan_journal_reader_from(
                &mock,
                JOURNAL_HEADER_LEN as u64,
                Arc::new(Vec::new()),
                &journal_path,
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn scan_retains_the_catalog_without_rereading_the_part() {
    let dir = tempfile::tempdir().unwrap();
    let journal_path = dir.path().join("active.wal");
    let p = part(2000);
    let data = journal(2_000, &[p]);
    let mock = RejectsRepeatedOffsets {
        data,
        seen: std::cell::RefCell::new(std::collections::HashSet::new()),
    };

    let local = LocalDir::open(dir.path()).unwrap();
    let scan = local
        .scan_journal_reader_from(
            &mock,
            JOURNAL_HEADER_LEN as u64,
            Arc::new(Vec::new()),
            &journal_path,
        )
        .unwrap();
    assert_eq!(scan.active.len(), 1);
    assert_eq!(scan.active[0].catalog.min_ts, 2_000);
}
