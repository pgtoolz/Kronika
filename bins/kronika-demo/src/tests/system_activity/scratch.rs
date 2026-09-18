use super::{SCRATCH_FILE_NAME, prepare};
use crate::system_activity::tests::config;
use std::os::unix::fs::{FileExt as _, symlink};

#[test]
fn positional_writes_wrap_without_growing_the_file() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let (mut ring, mut cleanup) = prepare(&config).unwrap();
    let page_size = u64::try_from(rustix::param::page_size()).unwrap();
    let pages = config.file_bytes().unwrap() / page_size;
    for _ in 0..=pages * 2 {
        ring.write_pages(page_size).unwrap();
        ring.sync_and_read().unwrap();
    }

    let path = cleanup.path().to_owned();
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        config.file_bytes().unwrap()
    );
    let mut first = [0_u8; 1];
    std::fs::File::open(&path)
        .unwrap()
        .read_exact_at(&mut first, 0)
        .unwrap();
    let expected = u8::try_from((pages * 2) % 256).unwrap().wrapping_add(1);
    assert_eq!(first[0], expected);
    cleanup.cleanup().unwrap();
    assert!(!path.exists());
}

#[test]
fn cleanup_removes_only_the_owned_file() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    std::fs::create_dir_all(&config.directory).unwrap();
    let sibling = config.directory.join("keep.txt");
    std::fs::write(&sibling, b"keep").unwrap();
    let (_ring, mut cleanup) = prepare(&config).unwrap();
    let scratch = cleanup.path().to_owned();

    cleanup.cleanup().unwrap();

    assert!(!scratch.exists());
    assert_eq!(std::fs::read(&sibling).unwrap(), b"keep");
    assert!(config.directory.exists());
}

#[test]
fn a_final_symlink_is_refused_without_touching_its_target() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    std::fs::create_dir_all(&config.directory).unwrap();
    let target = root.path().join("target");
    std::fs::write(&target, b"untouched").unwrap();
    symlink(&target, config.directory.join(SCRATCH_FILE_NAME)).unwrap();

    let error = prepare(&config).err().unwrap().to_string();

    assert!(error.contains("not a regular file"));
    assert_eq!(std::fs::read(target).unwrap(), b"untouched");
}

#[test]
fn a_directory_symlink_cannot_redirect_scratch_into_storage() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let link = root.path().join("redirected-activity");
    symlink(&config.storage_directory, &link).unwrap();
    config.directory = link;

    let error = prepare(&config).err().unwrap().to_string();

    assert!(error.contains("outside KRONIKA_STORAGE_DIR"));
}
