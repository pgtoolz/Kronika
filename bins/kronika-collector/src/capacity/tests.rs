use super::{RECORD_LEN, decode_records, is_local_filesystem, write_record};
use kronika_source_os::FsSpace;

#[test]
fn local_filesystem_allowlist_is_explicit() {
    for fstype in [
        "ext2", "ext3", "ext4", "xfs", "btrfs", "f2fs", "zfs", "tmpfs", "overlay",
    ] {
        assert!(is_local_filesystem(fstype), "{fstype}");
    }
    for fstype in [
        "nfs",
        "nfs4",
        "cifs",
        "fuse",
        "fuse.sshfs",
        "autofs",
        "mysteryfs",
    ] {
        assert!(!is_local_filesystem(fstype), "{fstype}");
    }
}

#[test]
fn completed_records_survive_an_incomplete_tail() {
    let expected = FsSpace {
        total_bytes: 100,
        free_bytes: 40,
        total_inodes: 20,
        available_inodes: 8,
    };
    let mut response = Vec::new();
    write_record(&mut response, Some(expected)).expect("complete record");
    response.extend_from_slice(&[1; RECORD_LEN - 1]);
    let eligible = [(1, "/one"), (2, "/two")];
    let mut capacities = vec![None; 3];

    let completed = decode_records(&response, &eligible, &mut capacities);

    assert_eq!(completed, 1);
    assert_eq!(capacities, vec![None, Some(expected), None]);
}
