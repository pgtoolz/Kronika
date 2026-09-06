use super::{FsSpace, ProcFs, SysFs, parse_dev_pair, parse_fixture, space_from_raw};
use std::io::Write;

#[test]
fn space_from_raw_normal() {
    let s = space_from_raw(1000, 400, 4096, 200, 80);
    assert_eq!(s.total_bytes, 1000 * 4096);
    assert_eq!(s.free_bytes, 400 * 4096);
    assert_eq!(s.total_inodes, 200);
    assert_eq!(s.available_inodes, 80);
}

#[test]
fn space_from_raw_overflow_saturates() {
    let s = space_from_raw(u64::MAX, u64::MAX, 4096, u64::MAX, u64::MAX);
    assert_eq!(s.total_bytes, i64::MAX);
    assert_eq!(s.free_bytes, i64::MAX);
    assert_eq!(s.total_inodes, i64::MAX);
    assert_eq!(s.available_inodes, i64::MAX);
}

#[test]
fn statvfs_fixture_hit() {
    assert_eq!(
        parse_fixture("/data=1000:400:200:80", "/data"),
        Some(FsSpace {
            total_bytes: 1000,
            free_bytes: 400,
            total_inodes: 200,
            available_inodes: 80,
        })
    );
}

#[test]
fn statvfs_fixture_miss() {
    assert_eq!(parse_fixture("/data=1000:400:200:80", "/other"), None);
}

#[test]
fn statvfs_fixture_multiple_entries() {
    let fixture = "/data=1000:400:200:80;/var=2048:512:300:120";
    assert_eq!(
        parse_fixture(fixture, "/var"),
        Some(FsSpace {
            total_bytes: 2048,
            free_bytes: 512,
            total_inodes: 300,
            available_inodes: 120,
        })
    );
    assert_eq!(parse_fixture(fixture, "/missing"), None);
}

#[test]
fn statvfs_fixture_malformed_entry_skipped() {
    // A malformed entry before the target must be skipped, not abort the scan.
    assert_eq!(
        parse_fixture("garbage;/data=1000:400:200:80", "/data"),
        Some(FsSpace {
            total_bytes: 1000,
            free_bytes: 400,
            total_inodes: 200,
            available_inodes: 80,
        })
    );
    // A miss still returns None even when malformed entries are present.
    assert_eq!(
        parse_fixture("garbage;/data=1000:400:200:80", "/other"),
        None
    );
}

#[test]
fn reads_relative_path_under_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("sys/kernel")).expect("mkdir");
    let mut f = std::fs::File::create(dir.path().join("sys/kernel/hostname")).expect("create");
    writeln!(f, "  probe-host  ").expect("write");
    let fs = ProcFs::new(dir.path().to_path_buf());
    assert_eq!(fs.read("sys/kernel/hostname").expect("read"), "probe-host");
}

#[test]
fn empty_file_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::File::create(dir.path().join("stat")).expect("create");
    let fs = ProcFs::new(dir.path().to_path_buf());
    assert!(fs.read("stat").is_err());
}

#[test]
fn missing_file_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = ProcFs::new(dir.path().to_path_buf());
    assert!(fs.read("nope").is_err());
}

#[test]
fn empty_relative_path_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = ProcFs::new(dir.path().to_path_buf());
    assert!(fs.read("").is_err(), "empty rel must not read the root dir");
    assert!(fs.read_raw("   ").is_err());
}

#[test]
fn parent_relative_path_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = ProcFs::new(dir.path().to_path_buf());
    assert!(fs.read_raw("../stat").is_err());
    assert!(fs.read_raw("/proc/stat").is_err());
}

#[test]
fn parse_dev_pair_reads_major_minor() {
    assert_eq!(parse_dev_pair("259:3\n"), Some((259, 3)));
    assert_eq!(parse_dev_pair("  8:1  "), Some((8, 1)));
}

#[test]
fn parse_dev_pair_rejects_malformed() {
    assert_eq!(parse_dev_pair("259"), None);
    assert_eq!(parse_dev_pair("a:b"), None);
    assert_eq!(parse_dev_pair(""), None);
}

#[test]
fn sysfs_reads_block_dev_under_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("class/block/dm-0")).expect("mkdir");
    std::fs::write(dir.path().join("class/block/dm-0/dev"), "253:0\n").expect("write");
    let sys = SysFs::new(dir.path().to_path_buf());
    assert_eq!(sys.read("class/block/dm-0/dev").expect("read"), "253:0");
}

#[test]
fn sysfs_trims_in_place_without_changing_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("value"), " \tprobe value\r\n ").expect("write value");
    let sys = SysFs::new(dir.path().to_path_buf());
    assert_eq!(sys.read("value").expect("read"), "probe value");
}

#[test]
fn sysfs_rejects_escape_and_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sys = SysFs::new(dir.path().to_path_buf());
    assert!(sys.read("../etc/passwd").is_err());
    assert!(sys.read("class/block/nope/dev").is_err());
}

#[test]
fn oversized_file_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("stat"),
        "x".repeat(super::MAX_PROC_FILE_BYTES + 1),
    )
    .expect("write large fixture");
    let fs = ProcFs::new(dir.path().to_path_buf());
    let err = fs.read_raw("stat").expect_err("oversized file rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::Other);
}
