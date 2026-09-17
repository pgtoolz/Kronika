use super::OsKernelLimits;
use crate::{Section, Ts, contract::lint};

#[test]
fn contract_passes_the_linter() {
    assert_eq!(lint(&[OsKernelLimits::CONTRACT]), Ok(()));
}

#[test]
fn roundtrip_keeps_absent_sources_absent() {
    let full = OsKernelLimits {
        ts: Ts(1),
        nr_file: Some(3_200),
        nr_free_file: Some(0),
        max_file: Some(9_223_372),
        nr_inode: Some(120_000),
        nr_free_inode: Some(1_000),
        nr_dentry: Some(300_000),
        nr_unused_dentry: Some(250_000),
        scope: 0,
    };
    let bare = OsKernelLimits {
        ts: Ts(2),
        nr_file: None,
        nr_free_file: None,
        max_file: None,
        nr_inode: None,
        nr_free_inode: None,
        nr_dentry: None,
        nr_unused_dentry: None,
        scope: 0,
    };
    crate::assert_roundtrips(&[full, bare]);
}
