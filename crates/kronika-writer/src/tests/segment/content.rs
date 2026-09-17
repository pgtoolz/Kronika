//! Content.

use super::*;

#[test]
fn writes_journal_parts_into_a_readable_segment() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let segment_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);

    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1_000);
    append_window(&mut journal, 2_000);

    let summary = write_segment(&journal, &owner, address()).expect("write the segment");
    assert_eq!(summary.sections, 1, "one topology section per segment");
    assert_eq!((summary.min_ts, summary.max_ts), (1_000, 2_000));

    // A chartless segment has the same container shape as a ZMS part.
    let segment = std::fs::read(&segment_path).expect("read segment");
    assert_eq!(u64::try_from(segment.len()).unwrap(), summary.bytes);
    let catalog = validate_part(&segment).expect("segment validates");
    assert_eq!(catalog.window_count, 2, "both collection windows");
    let [entry] = catalog.entries.as_slice() else {
        panic!("the finished segment must coalesce to one section");
    };
    assert_eq!(entry.type_id, 1_113_001);
    let start = usize::try_from(entry.offset).unwrap();
    let len = usize::try_from(entry.len).unwrap();
    let body = Bytes::copy_from_slice(&segment[start..start + len]);
    let verified =
        VerifiedSection::verify(body, entry.crc32c, crc32c).expect("section crc matches");
    assert_eq!(
        decode_any(1_113_001, verified).expect("decode").stats.rows,
        2
    );
}

#[test]
fn aggregate_list_values_cross_the_former_row_derived_cap() {
    use kronika_registry::pg_locks::PgLocksV2;

    fn wide_lock(ts: i64, pid: i32) -> PgLocksV2 {
        PgLocksV2 {
            ts: Ts(ts),
            pid,
            blocked_by: vec![pid; 4_096],
            datid: 16_384,
            datname: StrId(1),
            usename: Some(StrId(2)),
            application_name: StrId(3),
            client_addr: StrId(4),
            backend_type: StrId(5),
            state: Some(StrId(6)),
            wait_event_type: None,
            wait_event: None,
            query: StrId(7),
            backend_xid_age: None,
            backend_xmin_age: None,
            backend_start: Some(Ts(ts - 60)),
            xact_start: Some(Ts(ts - 5)),
            query_start: Some(Ts(ts - 1)),
            state_change: Some(Ts(ts - 1)),
            lock_locktype: None,
            lock_mode: None,
            lock_database: None,
            lock_relation: None,
            lock_relname: None,
            lock_page: None,
            lock_tuple: None,
            lock_virtualxid: None,
            lock_transactionid: None,
            lock_classid: None,
            lock_objid: None,
            lock_objsubid: None,
            lock_target: None,
            waitstart: None,
        }
    }

    fn append_lock_window(journal: &mut Journal, ts: i64, first_pid: i32) {
        let mut buffers = SectionBuffers::new();
        for row in 0..33 {
            buffers
                .push(wide_lock(ts + i64::from(row), first_pid + row))
                .expect("lock row fits");
        }
        let part = buffers.flush(&[]).expect("encode").expect("a part");
        journal
            .append(address().id, &part)
            .expect("append lock window");
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let journal_path = dir.path().join(ACTIVE_JOURNAL_NAME);
    let segment_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_lock_window(&mut journal, 1_000, 1);
    append_lock_window(&mut journal, 2_000, 100);
    let journal_before = std::fs::read(&journal_path).expect("snapshot journal");

    let summary = write_segment(&journal, &owner, address())
        .expect("the aggregate list stream fits the physical section envelope");
    assert_eq!(summary.sections, 1);
    assert_eq!(
        std::fs::read(&journal_path).expect("read journal"),
        journal_before
    );
    assert_eq!(journal.parts().len(), 2);
    let segment = std::fs::read(&segment_path).expect("read finished segment");
    let catalog = validate_part(&segment).expect("finished segment validates");
    let [entry] = catalog.entries.as_slice() else {
        panic!("the finished segment must coalesce the two lock windows");
    };
    assert_eq!(entry.type_id, PgLocksV2::CONTRACT.type_id.get());
    assert_eq!(entry.rows, 66);
}

#[test]
fn writing_an_empty_journal_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    assert!(matches!(
        write_segment(&journal, &owner, address()),
        Err(WriteError::Empty)
    ));
}

#[test]
fn same_inode_rewrite_during_recovery_comparison_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1);
    write_segment(&journal, &owner, address()).expect("first write");

    let path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let before_file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open final");
    let before = FileIdentity::from_file(&before_file).expect("initial identity");
    let path_for_hook = path;
    let hook = arm_after_first_comparison_chunk(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path_for_hook)
            .expect("open final for rewrite");
        let original_modified = file.metadata().unwrap().modified().unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact_at(&mut byte, MAGIC.len() as u64)
            .expect("read compared byte");
        file.write_all_at(&[byte[0] ^ 0xFF], MAGIC.len() as u64)
            .expect("rewrite compared byte");
        file.write_all_at(&byte, MAGIC.len() as u64)
            .expect("restore compared byte");
        file.set_times(FileTimes::new().set_modified(original_modified))
            .expect("restore mtime");
        file.sync_all().expect("persist restored content");
        assert_ne!(
            FileIdentity::from_file(&file).expect("changed identity"),
            before,
            "ctime must expose a rewrite even after restoring bytes and mtime"
        );
    });

    assert!(matches!(
        write_segment(&journal, &owner, address()),
        Err(WriteError::ExistingSegmentMismatch)
    ));
    hook.assert_consumed();
    assert_eq!(journal.parts().len(), 1, "journal must not be reset");
}

#[test]
fn final_name_replacement_during_recovery_comparison_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_window(&mut journal, 1);
    write_segment(&journal, &owner, address()).expect("first write");

    let path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let replacement_bytes = std::fs::read(&path).expect("read final");
    let displaced = path.with_extension("zms.displaced");
    let path_for_hook = path;
    let hook = arm_after_first_comparison_chunk(move || {
        std::fs::rename(&path_for_hook, &displaced).expect("displace final name");
        std::fs::write(&path_for_hook, &replacement_bytes)
            .expect("replace with byte-identical inode");
        std::fs::OpenOptions::new()
            .read(true)
            .open(&path_for_hook)
            .unwrap()
            .sync_all()
            .expect("persist replacement");
    });

    assert!(matches!(
        write_segment(&journal, &owner, address()),
        Err(WriteError::ExistingSegmentMismatch)
    ));
    hook.assert_consumed();
    assert_eq!(journal.parts().len(), 1, "journal must not be reset");
}

#[test]
fn catalog_entry_limit_is_checked_without_allocating_the_limit() {
    assert_eq!(
        checked_catalog_entries(MAX_CATALOG_ENTRIES - 1, 1).unwrap(),
        MAX_CATALOG_ENTRIES
    );
    assert!(matches!(
        checked_catalog_entries(MAX_CATALOG_ENTRIES, 1),
        Err(WriteError::CatalogTooLarge {
            attempted_entries,
            max_entries
        }) if attempted_entries == MAX_CATALOG_ENTRIES + 1
            && max_entries == MAX_CATALOG_ENTRIES
    ));
    assert!(matches!(
        checked_catalog_entries(usize::MAX, 1),
        Err(WriteError::CatalogTooLarge {
            attempted_entries: usize::MAX,
            ..
        })
    ));
}
