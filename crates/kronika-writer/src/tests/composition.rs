//! Cross-crate check: a part built by `kronika-format` survives the
//! file-backed journal unchanged.

use kronika_format::{PartMeta, SectionInput, build_part, validate_part};
use kronika_layout::{DataRoot, LayoutLimits, SegmentId};

use crate::{Journal, JournalConfig};

#[test]
fn a_built_part_survives_the_file_journal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(dir.path()).expect("open root");
    let owner = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let segment_id = SegmentId::new(1_709_164_800_000_000).expect("segment id");

    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 2,
                body: b"loadavg-section-body",
            },
            SectionInput {
                type_id: 1_021_001,
                rows: 1,
                body: b"instance-metadata-body",
            },
        ],
        PartMeta {
            min_ts: 1_000,
            max_ts: 2_000,
        },
    );

    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open");
    let part_ref = journal
        .append(segment_id, &part)
        .expect("append a valid part");

    let read_back = journal.read_part(part_ref).expect("read the part back");
    assert_eq!(read_back, part, "the journal returns the bytes appended");

    let catalog = validate_part(&read_back).expect("the persisted part validates");
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!(catalog.entries[0].type_id, 1_006_001);
}
