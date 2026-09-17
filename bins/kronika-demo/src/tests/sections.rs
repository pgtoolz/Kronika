use super::measure;
use kronika_format::{Catalog, Entry, FORMAT_VERSION};

fn segment(rows: u32) -> Vec<u8> {
    Catalog {
        entries: vec![Entry {
            type_id: 2_001_001,
            flags: 0,
            offset: 0,
            len: 0,
            rows,
            crc32c: 0,
        }],
        min_ts: 0,
        max_ts: 0,
        format_version: FORMAT_VERSION,
        window_count: 1,
    }
    .encode()
}

#[test]
fn one_scan_counts_segments_bytes_and_section_rows() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("history")).unwrap();
    let first = segment(4);
    let second = segment(8);
    std::fs::write(root.path().join("one.zms"), &first).unwrap();
    std::fs::write(root.path().join("history/two.zms"), &second).unwrap();
    std::fs::write(root.path().join("active.wal"), [0; 16]).unwrap();
    let summary = measure(root.path()).unwrap();
    assert_eq!(summary.count, 2);
    assert_eq!(
        summary.bytes,
        u64::try_from(first.len() + second.len()).unwrap()
    );
    assert_eq!(summary.sections.len(), 1);
    assert_eq!(summary.sections[0].rows, 12);
    assert_eq!(summary.sections[0].name, "pg_log_errors");
    assert_eq!(measure(&root.path().join("missing")).unwrap().count, 0);
}

#[test]
fn invalid_segment_is_reported_and_directory_symlinks_are_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("broken.zms"), b"invalid").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("other")).unwrap();
    assert_eq!(measure(root.path()).unwrap().count, 0);
    assert!(measure(outside.path()).is_err());
}
