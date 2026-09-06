//! Reset.

use super::*;

#[test]
fn reset_empties_the_journal() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    journal.append(id(1_000), &sample_part()).expect("append");
    journal.reset().expect("reset");
    assert!(journal.is_empty());
    assert_eq!(journal.len(), JOURNAL_HEADER_LEN);
}

#[test]
fn a_reference_is_stale_after_reset_even_when_raw_location_is_reused() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let segment_id = id(1_000);
    let stale = journal.append(segment_id, &sample_part()).unwrap();
    journal.reset().unwrap();

    let replacement = sample_part_with_section(b"more");
    let fresh = journal.append(segment_id, &replacement).unwrap();
    assert_eq!(stale.offset(), fresh.offset());
    assert_eq!(stale.len(), fresh.len());
    assert_ne!(stale.generation, fresh.generation);
    assert!(matches!(
        journal.read_part(stale),
        Err(JournalError::StalePartRef { .. })
    ));
    assert_eq!(journal.read_part(fresh).unwrap(), replacement);
}

#[test]
fn a_valid_marker_does_not_reset_a_damaged_previous_body() {
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
    let section_at = journal.parts()[0].offset() as u64 + MAGIC.len() as u64;
    journal.file.write_all_at(&[0xFF], section_at).unwrap();
    journal.file.sync_data().unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    drop(journal);

    assert!(Journal::open(&owner, JournalConfig::default()).is_err());
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}
