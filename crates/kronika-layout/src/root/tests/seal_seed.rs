//! Persistent seal seed ownership, format, and publication recovery.

use super::*;

const KNOWN_SEED: &[u8; 16] = b"KSEED\0\0\x01\xef\xcd\xab\x89\x67\x45\x23\x01";

#[test]
fn seal_seed_survives_repeated_loads_and_writer_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let owner = root.acquire_writer(LayoutLimits::default()).unwrap();
    let seed = owner.load_or_create_seal_seed().unwrap();
    let path = directory.path().join(SEAL_SEED_NAME);
    let persisted = std::fs::read(&path).unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();
    assert_eq!(&persisted[..8], &KNOWN_SEED[..8]);
    assert_eq!(&persisted[8..], &seed.to_le_bytes());
    assert_eq!(owner.load_or_create_seal_seed().unwrap(), seed);
    assert!(!directory.path().join(SEAL_SEED_TEMP_NAME).exists());
    assert!(matches!(
        DataRoot::open(directory.path())
            .unwrap()
            .acquire_writer(LayoutLimits::default()),
        Err(LayoutError::OwnerContended {
            owner: OwnerKind::Writer
        })
    ));
    drop(owner);
    drop(root);

    let restarted = DataRoot::open(directory.path())
        .unwrap()
        .acquire_writer(LayoutLimits::default())
        .unwrap();
    assert_eq!(restarted.load_or_create_seal_seed().unwrap(), seed);
    assert_eq!(std::fs::read(&path).unwrap(), persisted);
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
}

#[test]
fn independent_roots_generate_distinct_seal_seeds() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let seeds = [first.path(), second.path()].map(|path| {
        DataRoot::open(path)
            .unwrap()
            .acquire_writer(LayoutLimits::default())
            .unwrap()
            .load_or_create_seal_seed()
            .unwrap()
    });
    assert_ne!(seeds[0], seeds[1]);
}

#[test]
fn seal_seed_uses_the_owned_root_descriptor_after_a_path_swap() {
    let parent = tempfile::tempdir().unwrap();
    let original = parent.path().join("root");
    let moved = parent.path().join("moved");
    std::fs::create_dir(&original).unwrap();
    let owner = DataRoot::open(&original)
        .unwrap()
        .acquire_writer(LayoutLimits::default())
        .unwrap();
    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();

    owner.load_or_create_seal_seed().unwrap();
    assert!(moved.join(SEAL_SEED_NAME).is_file());
    assert!(!original.join(SEAL_SEED_NAME).exists());
    assert!(!moved.join(SEAL_SEED_TEMP_NAME).exists());
}

#[test]
fn malformed_seal_seed_is_reported_without_replacing_it_or_its_temporary() {
    let mut malformed: Vec<Vec<u8>> = (0..16).map(|len| KNOWN_SEED[..len].to_vec()).collect();
    malformed.push([KNOWN_SEED.as_slice(), &[0]].concat());
    malformed.push(vec![0; 16]);
    let mut unknown_version = *KNOWN_SEED;
    unknown_version[7] = 2;
    malformed.push(unknown_version.to_vec());
    for bytes in malformed {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SEAL_SEED_NAME);
        let temporary = directory.path().join(SEAL_SEED_TEMP_NAME);
        std::fs::write(&path, &bytes).unwrap();
        std::fs::write(&temporary, b"unfinished").unwrap();
        let owner = DataRoot::open(directory.path())
            .unwrap()
            .acquire_writer(LayoutLimits::default())
            .unwrap();
        assert!(matches!(
            owner.load_or_create_seal_seed(),
            Err(LayoutError::InvalidSealSeed)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read(&temporary).unwrap(), b"unfinished");
    }
}

#[test]
fn oversized_seal_seed_is_rejected_before_reading_its_contents() {
    let directory = tempfile::tempdir().unwrap();
    File::create(directory.path().join(SEAL_SEED_NAME))
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    let owner = DataRoot::open(directory.path())
        .unwrap()
        .acquire_writer(LayoutLimits::default())
        .unwrap();
    assert!(matches!(
        owner.load_or_create_seal_seed(),
        Err(LayoutError::InvalidSealSeed)
    ));
}

#[test]
fn interrupted_seal_seed_publication_discards_only_the_reserved_regular_temporary() {
    for final_exists in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SEAL_SEED_NAME);
        let temporary = directory.path().join(SEAL_SEED_TEMP_NAME);
        let unrelated = directory.path().join("seal.seed.other.tmp");
        std::fs::write(&unrelated, b"foreign").unwrap();
        if final_exists {
            std::fs::write(&path, KNOWN_SEED).unwrap();
            // A crash between link publication and temporary removal leaves both names.
            std::fs::hard_link(&path, &temporary).unwrap();
        } else {
            std::fs::write(&temporary, b"KSEED").unwrap();
        }
        let owner = DataRoot::open(directory.path())
            .unwrap()
            .acquire_writer(LayoutLimits::default())
            .unwrap();
        let seed = owner.load_or_create_seal_seed().unwrap();
        if final_exists {
            assert_eq!(seed, 0x0123_4567_89ab_cdef);
            assert_eq!(std::fs::read(&path).unwrap(), KNOWN_SEED);
        }
        assert_eq!(owner.load_or_create_seal_seed().unwrap(), seed);
        assert!(!temporary.exists());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"foreign");
    }
}

#[test]
fn seal_seed_control_symlinks_are_neither_followed_nor_removed() {
    for name in [SEAL_SEED_NAME, SEAL_SEED_TEMP_NAME] {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target");
        std::fs::write(&target, KNOWN_SEED).unwrap();
        let link = directory.path().join(name);
        symlink(&target, &link).unwrap();
        let owner = DataRoot::open(directory.path())
            .unwrap()
            .acquire_writer(LayoutLimits::default())
            .unwrap();
        assert!(matches!(
            owner.load_or_create_seal_seed(),
            Err(LayoutError::SymlinkNotAllowed { .. })
        ));
        assert!(link.is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), KNOWN_SEED);
        let snapshot = owner.root().scan(LayoutLimits::default()).unwrap();
        assert_eq!(snapshot.foreign_entries.len(), 1);
        assert_eq!(
            snapshot.foreign_entries[0].diagnostic().reason,
            ForeignEntryReason::SymbolicLink
        );
    }
}

#[test]
fn seal_seed_controls_reject_directories_and_fifos_without_blocking() {
    for name in [SEAL_SEED_NAME, SEAL_SEED_TEMP_NAME] {
        for fifo in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(name);
            if fifo {
                rustix::fs::mkfifoat(rustix::fs::CWD, &path, DATA_FILE_MODE).unwrap();
            } else {
                std::fs::create_dir(&path).unwrap();
            }
            let owner = DataRoot::open(directory.path())
                .unwrap()
                .acquire_writer(LayoutLimits::default())
                .unwrap();
            assert!(matches!(
                owner.load_or_create_seal_seed(),
                Err(LayoutError::UnexpectedLeafEntryType { .. })
            ));
            assert_eq!(
                owner
                    .root()
                    .scan(LayoutLimits::default())
                    .unwrap()
                    .foreign_entries
                    .len(),
                1
            );
            assert!(path.exists());
        }
    }
}

#[test]
fn read_only_scan_accepts_only_exact_regular_seal_seed_controls() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(SEAL_SEED_NAME), KNOWN_SEED).unwrap();
    std::fs::write(directory.path().join(SEAL_SEED_TEMP_NAME), b"unfinished").unwrap();
    let root = DataRoot::open(directory.path()).unwrap();
    let snapshot = root.scan(LayoutLimits::default()).unwrap();
    assert_eq!(snapshot.visited_entries, 2);
    assert!(snapshot.foreign_entries.is_empty());
    assert!(snapshot.temporaries.is_empty());
    assert!(snapshot.segments.is_empty());
    for name in ["seal.seed.old", "seal.seed.tmp.extra", ".seal.seed"] {
        std::fs::write(directory.path().join(name), b"foreign").unwrap();
    }
    assert_eq!(
        root.scan(LayoutLimits::default())
            .unwrap()
            .foreign_entries
            .len(),
        3
    );
    assert_eq!(
        std::fs::read(directory.path().join(SEAL_SEED_TEMP_NAME)).unwrap(),
        b"unfinished"
    );
}
