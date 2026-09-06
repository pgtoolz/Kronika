use super::*;
use crate::ProcFs;

const IO: ProcIo = ProcIo {
    rchar: 10,
    wchar: 20,
    syscr: 1,
    syscw: 2,
    read_bytes: 30,
    write_bytes: 40,
    cancelled_write_bytes: 0,
};

fn other(credentials: FsCredentials) -> FsCredentials {
    FsCredentials {
        uid: credentials.uid.wrapping_add(1),
        gid: credentials.gid.wrapping_add(1),
    }
}

fn target(pid: i32, credentials: FsCredentials) -> ProcessIoTarget {
    ProcessIoTarget::new(pid, credentials.uid, credentials.gid)
}

#[test]
fn warm_group_reuses_one_guard_and_a_failed_entry_learns_the_alternate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = ProcFs::new(dir.path().to_path_buf());
    let mut reader = ProcessReader::new(&fs);
    let mut cache = ProcessIoCredentials::new();
    let cached = other(cache.baseline);
    cache.by_pid.insert(10, cached);
    cache.by_pid.insert(11, cached);
    reset_test_io([IoRead::Value(IO), IoRead::Value(IO)]);
    let mut rows = 0;

    assert_eq!(
        cache.read(
            &mut reader,
            &[target(10, cache.baseline), target(11, cache.baseline)],
            |_, _| rows += 1,
        ),
        0
    );
    assert_eq!((rows, test_io_counts()), (2, (1, 0)));

    reset_test_io([IoRead::Unavailable, IoRead::Value(IO)]);
    assert_eq!(
        cache.read(&mut reader, &[target(10, cache.baseline)], |_, _| {}),
        0
    );
    assert_eq!(cache.by_pid.get(&10), Some(&cache.baseline));
}

#[test]
fn real_read_is_cached_unreadable_is_evicted_and_absent_pids_are_pruned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let process = dir.path().join("12");
    std::fs::create_dir(&process).expect("create process");
    std::fs::write(process.join("io"), "read_bytes: 30\n").expect("write io");
    let fs = ProcFs::new(dir.path().to_path_buf());
    let mut reader = ProcessReader::new(&fs);
    let mut cache = ProcessIoCredentials::new();
    let target = target(12, cache.baseline);
    reset_test_io([]);

    assert_eq!(cache.read(&mut reader, &[target], |_, _| {}), 0);
    cache.by_pid.insert(99, cache.baseline);
    cache.retain_live(&[12]);
    assert!(!cache.by_pid.contains_key(&99));

    std::fs::remove_file(process.join("io")).expect("remove io");
    std::fs::create_dir(process.join("io")).expect("make io unreadable");
    assert_eq!(cache.read(&mut reader, &[target], |_, _| {}), 1);
    assert!(!cache.by_pid.contains_key(&12));
}

#[test]
fn credential_guard_restores_the_calling_thread() {
    let before = current_fs_credentials();
    reset_test_io([]);
    {
        let _guard = FsCredGuard::switch(other(before));
    }
    assert_eq!(current_fs_credentials(), before);
}
