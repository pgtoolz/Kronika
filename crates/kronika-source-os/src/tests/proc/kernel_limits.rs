use super::{KernelLimitsRow, parse_kernel_limits};

#[test]
fn reads_every_field_from_the_three_files() {
    let row = parse_kernel_limits(
        Some("3200\t0\t9223372\n"),
        Some("120000\t1000\n"),
        Some("300000\t250000\t45\t0\t0\t0\n"),
    );
    assert_eq!(row.nr_file, Some(3_200));
    assert_eq!(row.nr_free_file, Some(0));
    assert_eq!(row.max_file, Some(9_223_372));
    assert_eq!(row.nr_inode, Some(120_000));
    assert_eq!(row.nr_free_inode, Some(1_000));
    assert_eq!(row.nr_dentry, Some(300_000));
    assert_eq!(row.nr_unused_dentry, Some(250_000));
}

#[test]
fn an_unreadable_file_leaves_its_fields_null() {
    let row = parse_kernel_limits(None, Some("1 2\n"), None);
    assert_eq!(row.nr_file, None);
    assert_eq!(row.max_file, None);
    assert_eq!(row.nr_inode, Some(1));
    assert_eq!(row.nr_dentry, None);
}

#[test]
fn a_truncated_or_garbled_file_yields_nulls_not_zeros() {
    assert_eq!(
        parse_kernel_limits(Some(""), None, None),
        KernelLimitsRow::default()
    );
    let row = parse_kernel_limits(Some("3200 broken 9223372\n"), None, None);
    assert_eq!(row.nr_file, Some(3_200));
    assert_eq!(row.nr_free_file, None);
    assert_eq!(row.max_file, Some(9_223_372));
}
