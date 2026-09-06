//! Header.

use super::*;

#[test]
fn fresh_journal_has_a_durable_empty_v1_header() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    assert!(journal.is_empty());
    assert_eq!(journal.len(), JOURNAL_HEADER_LEN);
    let bytes = std::fs::read(directory.path().join("active.wal")).unwrap();
    assert_eq!(
        JournalHeader::decode(bytes.try_into().unwrap()).unwrap(),
        JournalHeader::EMPTY
    );
}

#[test]
fn exact_header_length_cap_admits_only_an_empty_journal() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let config = JournalConfig {
        max_journal_len: JOURNAL_HEADER_LEN,
        ..JournalConfig::default()
    };
    let mut journal = Journal::open(&owner, config).unwrap();
    assert!(journal.is_empty());
    assert!(matches!(
        journal.append(id(1_000), &sample_part()),
        Err(JournalError::Full {
            len: JOURNAL_HEADER_LEN,
            max: JOURNAL_HEADER_LEN
        })
    ));
    assert_eq!(
        std::fs::metadata(directory.path().join("active.wal"))
            .unwrap()
            .len(),
        JOURNAL_HEADER_LEN as u64
    );
}

#[test]
fn every_first_append_write_and_sync_fault_reopens_as_empty() {
    const INJECTED_EIO: i32 = 5;
    for point in [
        JournalFaultPoint::AppendHeaderWrite,
        JournalFaultPoint::AppendFrameHeaderWrite,
        JournalFaultPoint::AppendFrameBodyWrite,
        JournalFaultPoint::AppendSync,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(&directory);
        let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
        let before = std::fs::read(directory.path().join("active.wal")).unwrap();
        let faults = arm_journal_faults([(point, INJECTED_EIO)]);

        let error = journal
            .append(id(1_000), &sample_part())
            .expect_err("an injected append fault cannot report success");
        assert_eq!(
            injected_operation_raw_os_error(&error),
            Some(INJECTED_EIO),
            "{point:?} must preserve the injected I/O error"
        );
        assert!(!journal.is_poisoned(), "{point:?} rollback must succeed");
        faults.assert_consumed();
        drop(journal);

        assert_eq!(
            std::fs::read(directory.path().join("active.wal")).unwrap(),
            before,
            "{point:?} must restore the exact empty journal"
        );
        let reopened = Journal::open(&owner, JournalConfig::default()).unwrap();
        assert!(reopened.is_empty(), "{point:?} invented an active append");
    }
}

#[test]
fn every_later_append_write_and_sync_fault_preserves_the_previous_generation() {
    const INJECTED_EIO: i32 = 5;
    for point in [
        JournalFaultPoint::AppendFrameHeaderWrite,
        JournalFaultPoint::AppendFrameBodyWrite,
        JournalFaultPoint::AppendHeaderWrite,
        JournalFaultPoint::AppendSync,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(&directory);
        let first = sample_part();
        let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
        journal.append(id(1_000), &first).unwrap();
        let before = std::fs::read(directory.path().join("active.wal")).unwrap();
        let faults = arm_journal_faults([(point, INJECTED_EIO)]);

        let error = journal
            .append(id(1_000), &sample_part_with_section(b"later"))
            .expect_err("an injected append fault cannot report success");
        assert_eq!(
            injected_operation_raw_os_error(&error),
            Some(INJECTED_EIO),
            "{point:?} must preserve the injected I/O error"
        );
        assert!(!journal.is_poisoned(), "{point:?} rollback must succeed");
        faults.assert_consumed();
        drop(journal);

        assert_eq!(
            std::fs::read(directory.path().join("active.wal")).unwrap(),
            before,
            "{point:?} must restore the exact previous generation"
        );
        let reopened = Journal::open(&owner, JournalConfig::default()).unwrap();
        assert_eq!(reopened.parts().len(), 1);
        assert_eq!(reopened.read_part(reopened.parts()[0]).unwrap(), first);
    }
}

#[test]
fn reset_writes_an_empty_header_instead_of_truncating_to_zero() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    journal.append(id(1_000), &sample_part()).unwrap();
    journal.reset().unwrap();
    assert_eq!(
        std::fs::metadata(directory.path().join("active.wal"))
            .unwrap()
            .len(),
        JOURNAL_HEADER_LEN as u64
    );
    assert_eq!(journal.segment_id(), None);
}

#[test]
fn every_reset_write_truncate_and_sync_fault_reopens_as_old_or_empty() {
    const INJECTED_EIO: i32 = 5;
    for (point, committed) in [
        (JournalFaultPoint::ResetMarkerWrite, false),
        (JournalFaultPoint::ResetMarkerSync, false),
        (JournalFaultPoint::ResetEmptyHeaderWrite, true),
        (JournalFaultPoint::ResetEmptyHeaderSync, true),
        (JournalFaultPoint::ResetTruncate, true),
        (JournalFaultPoint::ResetFinalSync, true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let owner = owner(&directory);
        let part = sample_part();
        let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
        journal.append(id(1_000), &part).unwrap();
        let active_before = std::fs::read(directory.path().join("active.wal")).unwrap();
        let faults = arm_journal_faults([(point, INJECTED_EIO)]);

        let error = journal
            .reset()
            .expect_err("an injected reset fault cannot report success");
        assert_eq!(
            injected_operation_raw_os_error(&error),
            Some(INJECTED_EIO),
            "{point:?} must preserve the injected I/O error"
        );
        assert_eq!(
            journal.is_poisoned(),
            committed,
            "{point:?} poison state must follow the reset commit boundary"
        );
        faults.assert_consumed();
        drop(journal);

        let reopened = Journal::open(&owner, JournalConfig::default()).unwrap();
        if committed {
            assert!(reopened.is_empty(), "{point:?} lost a committed reset");
            assert_eq!(reopened.len(), JOURNAL_HEADER_LEN);
        } else {
            assert_eq!(
                std::fs::read(directory.path().join("active.wal")).unwrap(),
                active_before,
                "{point:?} must restore the exact active generation"
            );
            assert_eq!(reopened.parts().len(), 1);
            assert_eq!(reopened.read_part(reopened.parts()[0]).unwrap(), part);
        }
    }
}

#[test]
fn open_completes_a_reset_after_the_root_header_was_partly_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_000);
    journal.append(segment_id, &sample_part()).unwrap();
    let previous_len = journal.end as u64;
    let marker = ResetMarker::new(previous_len, segment_id.get())
        .unwrap()
        .encode();
    journal
        .file
        .write_all_at(&marker, previous_len)
        .expect("write committed marker");
    journal.file.sync_data().expect("commit reset marker");
    journal
        .file
        .write_all_at(&JournalHeader::EMPTY.encode()[..17], 0)
        .expect("simulate interrupted root-header replacement");
    journal.file.sync_data().expect("persist interrupted state");
    drop(journal);

    let recovered = Journal::open(&owner, JournalConfig::default()).unwrap();
    assert!(recovered.is_empty());
    assert_eq!(recovered.len(), JOURNAL_HEADER_LEN);
}

#[test]
fn a_marker_with_an_inconsistent_previous_header_is_not_a_reset_commit() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_000);
    journal.append(segment_id, &sample_part()).unwrap();
    let previous_len = journal.end as u64;
    let forged = ResetMarker {
        previous_len,
        previous_segment_id: segment_id.get(),
        previous_header_crc: 0,
    }
    .encode();
    journal.file.write_all_at(&forged, previous_len).unwrap();
    journal.file.sync_data().unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    drop(journal);

    assert!(Journal::open(&owner, JournalConfig::default()).is_err());
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn a_valid_marker_does_not_reset_an_unrelated_header() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_000);
    journal.append(segment_id, &sample_part()).unwrap();
    let previous_len = journal.end as u64;
    let marker = ResetMarker::new(previous_len, segment_id.get())
        .unwrap()
        .encode();
    journal.file.write_all_at(&marker, previous_len).unwrap();
    journal.file.write_all_at(b"NOT-V1!!", 0).unwrap();
    journal.file.sync_data().unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    drop(journal);

    assert!(matches!(
        Journal::open(&owner, JournalConfig::default()),
        Err(JournalError::UnsupportedJournalFormat)
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn zero_length_journal_is_reinitialized_to_the_empty_header() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("active.wal"), []).unwrap();
    let owner = owner(&directory);
    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    assert!(journal.segment_id().is_none());
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        JournalHeader::EMPTY.encode()
    );
}

#[test]
fn headerless_journal_is_rejected_without_mutation() {
    let bytes = b"ZMSPheaderless".to_vec();
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("active.wal"), &bytes).unwrap();
    let owner = owner(&directory);
    assert!(matches!(
        Journal::open(&owner, JournalConfig::default()),
        Err(JournalError::TornHeader { .. })
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        bytes
    );
}

#[test]
fn bad_header_checksum_is_fatal_and_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    drop(journal);
    let file = OpenOptions::new()
        .write(true)
        .open(directory.path().join("active.wal"))
        .unwrap();
    file.write_all_at(&[0xAA], 16).unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    assert!(matches!(
        Journal::open(&owner, JournalConfig::default()),
        Err(JournalError::InvalidHeader(
            JournalHeaderError::BadChecksum { .. }
        ))
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn reset_final_sync_failure_reopens_as_logically_empty() {
    const INJECTED_EIO: i32 = 5;
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = owner(&directory);
    let part = sample_part();
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    let stale = journal.append(id(1_000), &part).expect("append");
    let faults = arm_journal_faults([(JournalFaultPoint::ResetFinalSync, INJECTED_EIO)]);

    let err = journal.reset().expect_err("sync failure is reported");
    assert_eq!(injected_operation_raw_os_error(&err), Some(INJECTED_EIO));
    assert!(journal.is_poisoned());
    assert!(matches!(
        journal.read_part(stale),
        Err(JournalError::Poisoned)
    ));
    faults.assert_consumed();
    drop(journal);

    let reopened = Journal::open(&owner, JournalConfig::default()).expect("reopen");
    assert!(reopened.is_empty());
    assert_eq!(reopened.len(), JOURNAL_HEADER_LEN);
}
