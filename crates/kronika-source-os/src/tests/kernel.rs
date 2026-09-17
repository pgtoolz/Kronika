use super::{collect_interrupts, collect_limits, collect_softirqs};
use crate::ProcFs;
use kronika_registry::StrId;

#[test]
fn partial_kernel_limits_report_errors_without_discarding_readable_sources() {
    let directory = tempfile::tempdir().expect("proc fixture");
    let root = directory.path();
    std::fs::create_dir_all(root.join("sys/fs/inode-nr")).expect("unreadable inode source");
    std::fs::write(root.join("sys/fs/file-nr"), "100 20 400\n").expect("file counters");
    let mut errors = Vec::new();
    let row = collect_limits(&ProcFs::new(root.to_path_buf()), 3, 41, |path, _error| {
        errors.push(path);
    })
    .expect("readable file counters");
    assert_eq!(errors, ["sys/fs/inode-nr"]);
    assert_eq!((row.ts.0, row.scope), (41, 3));
    assert_eq!(
        (row.nr_file, row.nr_free_file, row.max_file),
        (Some(100), Some(20), Some(400))
    );
    assert_eq!((row.nr_inode, row.nr_dentry), (None, None));
}

#[test]
fn interrupt_string_rejection_is_row_local_and_keeps_source_order() {
    let directory = tempfile::tempdir().expect("proc fixture");
    std::fs::write(
        directory.path().join("interrupts"),
        "           CPU0 CPU1\n  1: 10 20 IO-APIC keyboard\n  2: 30 40 IO-APIC disk\n",
    )
    .expect("interrupt counters");
    std::fs::write(
        directory.path().join("softirqs"),
        "           CPU0 CPU1\n TIMER: 3 4\n NET_RX: 5 6\n",
    )
    .expect("softirq counters");
    let fs = ProcFs::new(directory.path().to_path_buf());
    let mut strings = Vec::new();
    let rows = collect_interrupts(&fs, 4, 9, 2, |value| {
        strings.push(value.to_owned());
        (value != "1").then_some(StrId(7))
    })
    .expect("interrupt rows");
    assert_eq!(strings, ["1", "2", "IO-APIC disk"]);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].ts.0, rows[0].scope, rows[0].count), (9, 4, 70));
    let rows = collect_softirqs(&fs, 4, 9, |value| (value != "TIMER").then_some(StrId(8)))
        .expect("softirq rows");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].vector, rows[0].count), (StrId(8), 11));
}
