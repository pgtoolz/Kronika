//! Publish.

use super::*;

#[test]
fn rewriting_the_same_journal_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1);

    let first = write_segment(&journal, &owner, address()).expect("first write");
    let path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let bytes = std::fs::read(&path).expect("read first segment");
    let second = write_segment(&journal, &owner, address()).expect("idempotent recovery");

    assert_eq!(second, first);
    assert_eq!(std::fs::read(path).expect("read retry"), bytes);
    assert_eq!(journal.parts().len(), 1, "write never resets the journal");
    assert_eq!(day_entry_names(&owner), [address().zms_name()]);
}

fn day_entry_names(owner: &WriterOwner) -> Vec<String> {
    let path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut names = std::fs::read_dir(path.parent().expect("segment has a day directory"))
        .expect("read day directory")
        .map(|entry| {
            entry
                .expect("read day entry")
                .file_name()
                .into_string()
                .expect("fixture entry name is UTF-8")
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

#[test]
fn different_existing_segment_preserves_the_recovery_conflict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1);
    write_segment(&journal, &owner, address()).expect("first write");

    let path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open published segment");
    file.write_all_at(&[0xFF], MAGIC.len() as u64)
        .expect("change one body byte without changing the catalog");
    file.sync_all().expect("persist conflicting bytes");

    assert!(matches!(
        write_segment(&journal, &owner, address()),
        Err(WriteError::ExistingSegmentMismatch)
    ));
}

#[test]
fn body_corruption_after_append_prevents_publication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1);

    let part_ref = journal.parts()[0];
    let part = journal.read_part(part_ref).expect("read valid part");
    let catalog = validate_part(&part).expect("valid appended part");
    let body_at = u64::try_from(part_ref.offset()).unwrap() + catalog.entries[0].offset;
    let journal_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(ACTIVE_JOURNAL_NAME))
        .expect("open journal for corruption");
    let mut original = [0_u8; 1];
    journal_file
        .read_exact_at(&mut original, body_at)
        .expect("read body byte");
    journal_file
        .write_all_at(&[original[0] ^ 0xFF], body_at)
        .expect("corrupt section body");
    journal_file.sync_all().expect("persist corruption");

    assert!(matches!(
        write_segment(&journal, &owner, address()),
        Err(WriteError::Codec(
            kronika_registry::CodecError::SectionCrcMismatch { .. }
        ))
    ));
    assert_eq!(journal.parts().len(), 1, "journal remains recoverable");
    assert!(
        !owner
            .root()
            .diagnostic_file_path(address(), kronika_layout::FileKind::Zms)
            .exists(),
        "a corrupt journal body must not be published"
    );
    assert!(day_entry_names(&owner).is_empty());
}
