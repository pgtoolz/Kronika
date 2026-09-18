use super::{MAX_USERS_PER_SEGMENT, SegmentUserNames};
use kronika_format::DictLimits;
use kronika_source_os::PasswdSnapshot;
use kronika_writer::Interner;

const PASSWD: &str =
    "root:x:0:0:root:/root:/bin/sh\npostgres:x:26:26::/var/lib/postgresql:/bin/false\n";

fn names(contents: &str) -> SegmentUserNames {
    let file = tempfile::NamedTempFile::new().expect("passwd fixture");
    std::fs::write(file.path(), contents).expect("write passwd fixture");
    let passwd = PasswdSnapshot::read(file.path()).expect("read passwd fixture");
    SegmentUserNames::with_passwd(passwd)
}

#[test]
fn deduplicates_real_and_effective_uids_until_append_is_recorded() {
    let mut names = names(PASSWD);
    let mut interner = Interner::new(DictLimits::default());
    for uid in [26, 26, 0, 999] {
        names.observe_user(0, uid);
    }
    let (first, pending) = names.prepare_rows(&mut interner, 1);
    assert_eq!(first.iter().map(|row| row.uid).collect::<Vec<_>>(), [0, 26]);

    names.observe_user(0, 26);
    let (retry, _) = names.prepare_rows(&mut interner, 2);
    assert_eq!(retry.len(), 2, "a failed append must remain retryable");

    names.confirm_written(&pending);
    for uid in [0, 26, 999] {
        names.observe_user(0, uid);
    }
    let (next, pending) = names.prepare_rows(&mut interner, 3);
    assert!(next.is_empty());
    assert!(pending.is_empty());
}

#[test]
fn partial_confirmation_preserves_other_scopes_and_later_observations() {
    let mut names = names(PASSWD);
    let mut interner = Interner::new(DictLimits::default());
    names.observe_user(0, 26);
    names.observe_user(4, 26);
    let (_, pending) = names.prepare_rows(&mut interner, 1);
    assert_eq!(pending, [(0, 26), (4, 26)]);

    names.confirm_written(&[(0, 26)]);
    names.observe_user(0, 26);
    names.observe_user(0, 0);
    let (rows, pending) = names.prepare_rows(&mut interner, 2);

    assert_eq!(pending, [(0, 0), (4, 26)]);
    assert_eq!(
        rows.iter()
            .map(|row| (row.scope, row.uid, row.ts.0))
            .collect::<Vec<_>>(),
        [(0, 0, 2), (4, 26, 2)]
    );
    names.confirm_written(&pending);
    assert!(names.prepare_rows(&mut interner, 3).0.is_empty());
}

#[test]
fn a_full_dictionary_leaves_the_user_pending_for_retry() {
    let mut names = names(PASSWD);
    let limits = DictLimits::new(8, 8)
        .expect("valid limits")
        .with_max_total_bytes(8)
        .expect("one username fits");
    let mut interner = Interner::new(limits);
    interner.intern(b"occupied").expect("fill dictionary");
    names.observe_user(4, 26);

    let (rows, pending) = names.prepare_rows(&mut interner, 1);
    assert!(rows.is_empty());
    assert!(
        pending.is_empty(),
        "unencoded users cannot be confirmed written"
    );
    interner
        .flush_window(|_| Ok::<(), ()>(()))
        .expect("flush dictionary");

    let (rows, pending) = names.prepare_rows(&mut interner, 2);
    assert_eq!(pending, [(4, 26)]);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].scope, rows[0].uid, rows[0].ts.0), (4, 26, 2));
}

#[test]
fn the_observation_limit_includes_written_and_missing_users() {
    let limit = u32::try_from(MAX_USERS_PER_SEGMENT).expect("user limit fits UID");
    let mut names = names(&format!("{PASSWD}late:x:{limit}:{limit}::/:/bin/false\n"));
    let mut interner = Interner::new(DictLimits::default());
    names.observe_user(0, 0);
    let (_, written) = names.prepare_rows(&mut interner, 1);
    names.confirm_written(&written);

    for uid in 1..limit {
        names.observe_user(0, uid);
    }
    let (_, pending) = names.prepare_rows(&mut interner, 2);
    assert_eq!(pending, [(0, 26)]);

    names.observe_user(0, limit);
    names.observe_user(4, 26);
    names.observe_user(0, 26);
    let (rows, pending) = names.prepare_rows(&mut interner, 3);

    assert_eq!(pending, [(0, 26)]);
    assert_eq!(
        rows.iter()
            .map(|row| (row.scope, row.uid))
            .collect::<Vec<_>>(),
        [(0, 26)]
    );
    names.confirm_written(&pending);
    assert!(names.prepare_rows(&mut interner, 4).0.is_empty());
}
