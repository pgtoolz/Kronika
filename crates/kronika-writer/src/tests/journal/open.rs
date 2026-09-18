//! Open.

use super::*;

#[test]
fn append_read_reopen_roundtrip() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = owner(&directory);
    let part = sample_part();
    let segment_id = id(1_000);

    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    let first = journal.append(segment_id, &part).expect("append");
    let second = journal.append(segment_id, &part).expect("append");
    assert_eq!(journal.parts(), &[first, second]);
    assert_eq!(journal.read_part(first).expect("read"), part);
    assert_eq!(
        journal
            .read_part_range(first, MAGIC.len(), 4)
            .expect("read range"),
        b"data"
    );
    assert!(matches!(
        journal.read_part_range(first, part.len() - 1, 2),
        Err(JournalError::StalePartRef { .. })
    ));

    drop(journal);
    let journal = Journal::open(&owner, JournalConfig::default()).expect("reopen");
    assert_eq!(journal.parts().len(), 2);
    assert_eq!(journal.read_part(journal.parts()[1]).expect("read"), part);
}

#[test]
fn incomplete_final_frame_is_rejected_and_preserved_on_open() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = owner(&directory);
    let path = directory.path().join(ACTIVE_JOURNAL_NAME);
    let part = sample_part();

    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    journal.append(id(1_000), &part).expect("append");
    drop(journal);

    let mut file = OpenOptions::new().append(true).open(&path).expect("raw");
    let partial_frame_header = FrameHeader {
        part_len: part.len() as u64,
    }
    .encode();
    file.write_all(&partial_frame_header).expect("write");
    file.write_all(&part[..part.len() / 2]).expect("write");
    drop(file);
    let before = std::fs::read(&path).expect("read damaged journal");

    assert!(matches!(
        Journal::open(&owner, JournalConfig::default()),
        Err(JournalError::BodyLengthMismatch { .. })
    ));
    assert_eq!(std::fs::read(path).expect("read preserved journal"), before);
}

#[test]
fn damaged_frame_is_rejected_and_preserved_on_open() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = owner(&directory);
    let path = directory.path().join(ACTIVE_JOURNAL_NAME);
    let part = sample_part();

    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    let part_ref = journal.append(id(1_000), &part).expect("append");
    drop(journal);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("raw");
    let frame_at = part_ref.offset() - FRAME_HEADER_LEN;
    file.write_all_at(&[0], frame_at as u64)
        .expect("damage frame magic");
    file.sync_all().expect("persist damage");
    let before = std::fs::read(&path).expect("read damaged journal");

    assert!(matches!(
        Journal::open(&owner, JournalConfig::default()),
        Err(JournalError::DamagedBody)
    ));
    assert_eq!(std::fs::read(path).expect("read preserved journal"), before);
}

#[test]
fn a_reference_from_a_previous_open_is_stale() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let part = sample_part();
    let mut journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let stale = journal.append(id(1_000), &part).unwrap();
    drop(journal);

    let journal = Journal::open(&owner, JournalConfig::default()).unwrap();
    let current = journal.parts()[0];
    assert_eq!(stale.offset(), current.offset());
    assert_eq!(stale.len(), current.len());
    assert_ne!(stale.generation, current.generation);
    assert!(matches!(
        journal.read_part(stale),
        Err(JournalError::StalePartRef { .. })
    ));
    assert_eq!(journal.read_part(current).unwrap(), part);
}

#[test]
fn open_completes_a_reset_committed_before_the_final_truncate() {
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
    drop(journal);

    let recovered = Journal::open(&owner, JournalConfig::default()).unwrap();
    assert!(recovered.is_empty());
    assert_eq!(recovered.len(), JOURNAL_HEADER_LEN);
}

#[test]
fn part_count_cap_applies_to_append_and_open() {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let config = JournalConfig {
        max_parts: 2,
        ..JournalConfig::default()
    };
    let mut journal = Journal::open(&owner, config).unwrap();
    let part = sample_part();
    journal.append(id(1_000), &part).unwrap();
    journal.append(id(1_000), &part).unwrap();
    let before = std::fs::read(directory.path().join("active.wal")).unwrap();
    assert!(matches!(
        journal.append(id(1_000), &part),
        Err(JournalError::TooManyParts { max: 2 })
    ));
    drop(journal);

    let strict = JournalConfig {
        max_parts: 1,
        ..JournalConfig::default()
    };
    assert!(matches!(
        Journal::open(&owner, strict),
        Err(JournalError::TooManyParts { max: 1 })
    ));
    assert_eq!(
        std::fs::read(directory.path().join("active.wal")).unwrap(),
        before
    );
}
