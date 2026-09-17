//! Durability.

use super::*;

#[test]
fn existing_journal_retries_root_sync_after_initialization_sync_failure() {
    const FIRST_ERROR: i32 = 5;
    const RETRY_ERROR: i32 = 116;

    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let path = directory.path().join("active.wal");

    let first_faults = arm_journal_faults([(JournalFaultPoint::OpenRootSync, FIRST_ERROR)]);
    let first_error = Journal::open(&owner, JournalConfig::default())
        .expect_err("the initial root sync failure must reject open");
    assert_eq!(
        injected_operation_raw_os_error(&first_error),
        Some(FIRST_ERROR)
    );
    first_faults.assert_consumed();
    assert_eq!(
        JournalHeader::decode(std::fs::read(&path).unwrap().try_into().unwrap()).unwrap(),
        JournalHeader::EMPTY,
        "the file was initialized before its root sync failed"
    );

    let retry_faults = arm_journal_faults([(JournalFaultPoint::OpenRootSync, RETRY_ERROR)]);
    let retry_error = Journal::open(&owner, JournalConfig::default())
        .expect_err("an existing valid journal must retry the root sync");
    assert_eq!(
        injected_operation_raw_os_error(&retry_error),
        Some(RETRY_ERROR)
    );
    retry_faults.assert_consumed();

    let journal = Journal::open(&owner, JournalConfig::default())
        .expect("the next retry proves root-entry durability");
    assert!(journal.is_empty());
}

#[test]
fn invalid_length_config_is_rejected_before_creating_the_journal() {
    assert!(matches!(
        rejected_config(JournalConfig {
            max_journal_len: JOURNAL_HEADER_LEN - 1,
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxJournalLen {
            value,
            minimum: JOURNAL_HEADER_LEN,
            maximum: MAX_JOURNAL_LEN,
        } if value == JOURNAL_HEADER_LEN - 1
    ));
    assert!(matches!(
        rejected_config(JournalConfig {
            max_journal_len: MAX_JOURNAL_LEN + 1,
            ..JournalConfig::default()
        }),
        JournalError::InvalidMaxJournalLen {
            value,
            minimum: JOURNAL_HEADER_LEN,
            maximum: MAX_JOURNAL_LEN,
        } if value == MAX_JOURNAL_LEN + 1
    ));
}

#[test]
fn a_fabricated_in_bounds_reference_is_rejected_before_reading() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let genuine = journal.append(id(1_000), &sample_part()).unwrap();
    let fabricated = JournalPartRef::new(
        PartRef {
            offset: genuine.offset() + 1,
            len: genuine.len() - 1,
        },
        journal.generation,
    );
    assert!(fabricated.offset() + fabricated.len() <= journal.len());
    assert!(matches!(
        journal.read_part(fabricated),
        Err(JournalError::StalePartRef { .. })
    ));
    assert!(journal.read_part(genuine).is_ok());
}

#[test]
fn another_segment_id_is_rejected_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    journal.append(id(1_000), &sample_part()).unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    assert!(matches!(
        journal.append(id(2_000), &sample_part()),
        Err(JournalError::SegmentIdMismatch { .. })
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}

#[test]
fn journal_keeps_writer_ownership_after_the_original_owner_is_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let first_root = DataRoot::open(directory.path()).unwrap();
    let second_root = DataRoot::open(directory.path()).unwrap();
    let owner = first_root.acquire_writer(LayoutLimits::default()).unwrap();
    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    drop(owner);

    assert!(matches!(
        second_root.acquire_writer(LayoutLimits::default()),
        Err(LayoutError::OwnerContended {
            owner: OwnerKind::Writer
        })
    ));
    drop(journal);
    second_root.acquire_writer(LayoutLimits::default()).unwrap();
}

#[test]
fn fabricated_v2_journal_identities_are_rejected_without_mutation() {
    let canonical = JournalHeader::EMPTY.encode();
    let mut magic_v2 = canonical;
    magic_v2[..8].copy_from_slice(b"KRNJNL2\0");
    let magic_v2_crc = crc32c(&magic_v2[..32]);
    magic_v2[32..].copy_from_slice(&magic_v2_crc.to_le_bytes());

    let mut version_v2 = canonical;
    version_v2[8..12].copy_from_slice(&2_u32.to_le_bytes());
    let version_v2_crc = crc32c(&version_v2[..32]);
    version_v2[32..].copy_from_slice(&version_v2_crc.to_le_bytes());

    for bytes in [magic_v2, version_v2] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("active.wal");
        std::fs::write(&path, bytes).unwrap();
        let before = std::fs::read(&path).unwrap();
        let owner = owner(&directory);

        assert!(matches!(
            Journal::open(&owner, JournalConfig::default()),
            Err(JournalError::UnsupportedJournalFormat)
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "unsupported journal identities must not trigger fallback or migration"
        );
    }
}
