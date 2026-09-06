mod discovery;
mod ownership;
mod publish;

use std::fs::{FileTimes, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{FileExt as _, symlink};
use std::time::SystemTime;

use super::*;
use crate::time::SegmentId;

fn address(value: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(value).unwrap()).unwrap()
}

fn rewrite_same_inode_with_restored_mtime(
    path: &Path,
    prepared_identity: FileIdentity,
    prepared_mtime: SystemTime,
    replacement: &[u8],
) -> FileIdentity {
    assert_eq!(replacement.len() as u64, prepared_identity.len);
    let rewritten = OpenOptions::new().write(true).open(path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        rewritten.write_all_at(replacement, 0).unwrap();
        rewritten
            .set_times(FileTimes::new().set_modified(prepared_mtime))
            .unwrap();
        rewritten.sync_all().unwrap();
        let identity = FileIdentity::from_file(&rewritten).unwrap();
        if (identity.ctime_seconds, identity.ctime_nanoseconds)
            != (
                prepared_identity.ctime_seconds,
                prepared_identity.ctime_nanoseconds,
            )
        {
            return identity;
        }
        assert!(
            Instant::now() < deadline,
            "the filesystem did not expose the same-inode rewrite through ctime"
        );
        std::thread::yield_now();
    }
}
