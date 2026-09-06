use kronika_layout::WriterOwner;
mod append;
mod durability;
mod header;
mod open;
mod reset;

use std::fs::OpenOptions;
use std::os::unix::fs::FileExt as _;

use kronika_format::{Catalog, Entry, FORMAT_VERSION, MAGIC, crc32c};
use kronika_layout::{ACTIVE_JOURNAL_NAME, DataRoot, LayoutLimits, OwnerKind};

use super::*;

fn sample_part_with_section(section: &[u8]) -> Vec<u8> {
    let mut part = Vec::new();
    part.extend_from_slice(&MAGIC);
    part.extend_from_slice(section);
    let catalog = Catalog {
        entries: vec![Entry {
            type_id: 1_006_001,
            flags: 0,
            offset: 4,
            len: section.len() as u64,
            rows: 1,
            crc32c: crc32c(section),
        }],
        min_ts: 1,
        max_ts: 2,
        format_version: FORMAT_VERSION,
        window_count: 1,
    };
    part.extend_from_slice(&catalog.encode());
    assert!(validate_part(&part).is_ok());
    part
}

fn sample_part() -> Vec<u8> {
    sample_part_with_section(b"data")
}

fn owner(directory: &tempfile::TempDir) -> WriterOwner {
    DataRoot::open(directory.path())
        .unwrap()
        .acquire_writer(LayoutLimits::default())
        .unwrap()
}

fn id(value: i64) -> SegmentId {
    SegmentId::new(value).unwrap()
}

fn rejected_config(config: JournalConfig) -> JournalError {
    let directory = tempfile::tempdir().unwrap();
    let owner = owner(&directory);
    let error = Journal::open(&owner, config).unwrap_err();
    assert!(!directory.path().join("active.wal").exists());
    error
}

fn injected_operation_raw_os_error(error: &JournalError) -> Option<i32> {
    match error {
        JournalError::Io(source) | JournalError::ResetIncomplete(source) => source.raw_os_error(),
        _ => None,
    }
}
