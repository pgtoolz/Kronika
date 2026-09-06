use super::{OFFSETS_FILE_NAME, Offsets};
use crate::tail::Position;

#[test]
fn a_saved_offset_comes_back_after_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut offsets = Offsets::load(dir.path()).expect("load");
    offsets.set(
        "postgresql",
        Position {
            dev: 66_310,
            inode: 42,
            offset: 4096,
        },
    );
    offsets.save().expect("save");

    let reloaded = Offsets::load(dir.path()).expect("reload");

    assert_eq!(
        reloaded.get("postgresql"),
        Position {
            dev: 66_310,
            inode: 42,
            offset: 4096,
        }
    );
}

#[test]
fn a_source_that_has_never_read_starts_at_the_beginning() {
    let dir = tempfile::tempdir().expect("tempdir");

    let offsets = Offsets::load(dir.path()).expect("load");

    assert_eq!(offsets.get("pgbouncer"), Position::default());
}

#[test]
fn a_damaged_line_costs_its_own_source_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join(OFFSETS_FILE_NAME),
        "postgresql 1 2 not-a-number\npgbouncer 3 4 512\n",
    )
    .expect("write");

    let offsets = Offsets::load(dir.path()).expect("load");

    assert_eq!(offsets.get("postgresql"), Position::default());
    assert_eq!(
        offsets.get("pgbouncer"),
        Position {
            dev: 3,
            inode: 4,
            offset: 512,
        }
    );
}
