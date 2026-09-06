//! Standalone finished-ZMS assembly.

use std::io::Write as _;

use super::*;

#[test]
fn standalone_core_writes_canonical_catalog_with_caller_window_count() {
    let source_dir = tempfile::tempdir().expect("source tempdir");
    let owner = writer(&source_dir);
    let source_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let (_interner, ids) = fixture_dictionary();
    append_lossless_part(
        &mut journal,
        &[lossless_topology(ids, 0.0_f64.to_bits(), true)],
        &[lossless_process(ids, 20, true)],
    );
    write_segment(&journal, &owner, address()).expect("write source segment");

    let source = std::fs::read(source_path).expect("read source segment");
    let source_catalog = validate_part(&source).expect("source validates");
    let spool_dir = tempfile::tempdir().expect("spool tempdir");
    let spool_path = spool_dir.path().join("sections.spool");
    let mut spool_out = std::fs::File::create(&spool_path).expect("create spool");
    let mut descriptors = Vec::new();
    let mut offset = 0_u64;
    for entry in source_catalog.entries.iter().rev() {
        let start = usize::try_from(entry.offset).expect("source offset fits");
        let len = usize::try_from(entry.len).expect("source length fits");
        let body = &source[start..start + len];
        spool_out.write_all(body).expect("write body to spool");
        descriptors.push(
            FinishedSection::new(entry.type_id, entry.rows, offset, entry.len, entry.crc32c)
                .expect("valid descriptor"),
        );
        offset += entry.len;
    }
    spool_out.sync_all().expect("sync spool");
    let spool = std::fs::File::open(spool_path).expect("open spool");
    let plan = FinishedZmsPlan::new(descriptors, 42, 84, 0).expect("valid plan");
    let mut output = Vec::new();
    let summary = write_finished_zms(&spool, &plan, &mut output).expect("assemble standalone ZMS");

    assert_eq!(summary.min_ts, 42);
    assert_eq!(summary.max_ts, 84);
    assert_eq!(summary.bytes, output.len() as u64);
    let catalog = validate_part(&output).expect("standalone ZMS validates");
    assert_eq!(catalog.window_count, 0);
    assert!(
        catalog
            .entries
            .windows(2)
            .all(|pair| pair[0].type_id < pair[1].type_id),
        "the final catalog is in canonical type order"
    );
}

#[test]
fn standalone_core_rejects_a_changed_spool_body() {
    let spool_dir = tempfile::tempdir().expect("spool tempdir");
    let spool_path = spool_dir.path().join("sections.spool");
    let (type_id, rows, body) = registered_section_body();
    std::fs::write(&spool_path, &body).expect("write spool");
    let spool = std::fs::File::open(spool_path).expect("open spool");
    let section = FinishedSection::new(type_id, rows, 0, body.len() as u64, crc32c(&body))
        .expect("valid descriptor");
    let plan = FinishedZmsPlan::new(vec![section], 1, 1, 0).expect("valid plan");

    let changed = FinishedSection::new(
        section.type_id(),
        section.rows(),
        section.offset(),
        section.len(),
        section.crc32c() ^ 1,
    )
    .expect("changed descriptor remains structurally valid");
    let changed_plan = FinishedZmsPlan::new(vec![changed], 1, 1, 0).expect("valid changed plan");
    assert!(matches!(
        write_finished_zms(&spool, &changed_plan, &mut Vec::new()),
        Err(WriteError::Codec(
            kronika_registry::CodecError::SectionCrcMismatch { .. }
        ))
    ));

    write_finished_zms(&spool, &plan, &mut Vec::new()).expect("unchanged spool assembles");
}

fn registered_section_body() -> (u32, u32, Vec<u8>) {
    let directory = tempfile::tempdir().expect("source tempdir");
    let owner = writer(&directory);
    let source_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let (_interner, ids) = fixture_dictionary();
    append_lossless_part(
        &mut journal,
        &[lossless_topology(ids, 0.0_f64.to_bits(), true)],
        &[lossless_process(ids, 20, true)],
    );
    write_segment(&journal, &owner, address()).expect("write source segment");
    let source = std::fs::read(source_path).expect("read source segment");
    let catalog = validate_part(&source).expect("source validates");
    let entry = catalog
        .entries
        .iter()
        .find(|entry| !matches!(entry.type_id, DICT_STRINGS_TYPE_ID | DICT_BLOBS_TYPE_ID))
        .expect("registered data section");
    let start = usize::try_from(entry.offset).expect("section offset fits");
    let len = usize::try_from(entry.len).expect("section length fits");
    (
        entry.type_id,
        entry.rows,
        source[start..start + len].to_vec(),
    )
}
