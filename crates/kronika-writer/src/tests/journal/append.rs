//! Append.

use super::*;

#[test]
fn invalid_part_count_config_is_rejected_before_creating_the_journal() {
    assert!(matches!(
        rejected_config(JournalConfig {
            max_parts: 0,
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxParts {
            value: 0,
            minimum: 1,
            maximum: MAX_JOURNAL_PARTS,
        }
    ));
    assert!(matches!(
        rejected_config(JournalConfig {
            max_parts: MAX_JOURNAL_PARTS + 1,
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxParts {
            value,
            minimum: 1,
            maximum: MAX_JOURNAL_PARTS,
        } if value == MAX_JOURNAL_PARTS + 1
    ));
}

#[test]
fn invalid_part_length_config_is_rejected_before_creating_the_journal() {
    assert!(matches!(
        rejected_config(JournalConfig {
            limits: JournalLimits { max_part_len: 0 },
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxPartLen {
            value: 0,
            minimum: 1,
            maximum: MAX_PART_LEN,
        }
    ));
    assert!(matches!(
        rejected_config(JournalConfig {
            limits: JournalLimits {
                max_part_len: MAX_PART_LEN + 1,
            },
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxPartLen {
            value,
            minimum: 1,
            maximum: MAX_PART_LEN,
        } if value == MAX_PART_LEN + 1
    ));
}

#[test]
fn first_append_persists_identity_and_frame_together() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_709_164_800_000_000);
    let part = sample_part();
    let part_ref = journal.append(segment_id, &part).unwrap();
    assert_eq!(journal.segment_id(), Some(segment_id));
    assert_eq!(journal.read_part(part_ref).unwrap(), part);
    drop(journal);

    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    assert_eq!(journal.segment_id(), Some(segment_id));
    assert_eq!(journal.parts().len(), 1);
}

#[test]
fn configured_length_cap_rejects_an_existing_larger_journal() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    journal.append(id(1_000), &sample_part()).unwrap();
    let len = journal.len();
    drop(journal);
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    let config = JournalConfig {
        max_journal_len: len - 1,
        ..JournalConfig::default()
    };

    assert!(matches!(
        Journal::open(&owner, config),
        Err(JournalError::JournalTooLarge { .. })
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn physical_length_cap_is_checked_before_committed_reset_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_000);
    journal.append(segment_id, &sample_part()).unwrap();
    let previous_len = journal.len() as u64;
    let marker = ResetMarker::new(previous_len, segment_id.get())
        .unwrap()
        .encode();
    journal.file.write_all_at(&marker, previous_len).unwrap();
    journal.file.sync_data().unwrap();
    drop(journal);

    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    let config = JournalConfig {
        max_journal_len: before.len() - 1,
        ..JournalConfig::default()
    };
    assert!(matches!(
        Journal::open(&owner, config),
        Err(JournalError::JournalTooLarge { len, max })
            if len == before.len() as u64 && max == before.len() - 1
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn configured_length_cap_applies_to_the_first_and_later_appends() {
    let part = sample_part();
    let one_frame_len = JOURNAL_HEADER_LEN + FRAME_HEADER_LEN + part.len();
    let reset_peak_len = one_frame_len + RESET_MARKER_LEN;

    let first_directory = tempfile::tempdir().unwrap();
    let first_owner = owner(&first_directory);
    let first_config = JournalConfig {
        max_journal_len: reset_peak_len - 1,
        ..JournalConfig::default()
    };
    let mut first = Journal::open(&first_owner, first_config).unwrap();
    assert!(matches!(
        first.append(id(1_000), &part),
        Err(JournalError::Full { .. })
    ));
    assert_eq!(first.len(), JOURNAL_HEADER_LEN);

    let later_directory = tempfile::tempdir().unwrap();
    let later_owner = owner(&later_directory);
    let later_config = JournalConfig {
        max_journal_len: reset_peak_len,
        ..JournalConfig::default()
    };
    let mut later = Journal::open(&later_owner, later_config).unwrap();
    later.append(id(1_000), &part).unwrap();
    let before = std::fs::read(later_directory.path().join("active.wal")).unwrap();
    assert!(matches!(
        later.append(id(1_000), &part),
        Err(JournalError::Full { .. })
    ));
    assert_eq!(
        std::fs::read(later_directory.path().join("active.wal")).unwrap(),
        before
    );
    later.reset().unwrap();
    assert_eq!(later.len(), JOURNAL_HEADER_LEN);
    assert_eq!(
        std::fs::metadata(later_directory.path().join("active.wal"))
            .unwrap()
            .len(),
        JOURNAL_HEADER_LEN as u64
    );
}
