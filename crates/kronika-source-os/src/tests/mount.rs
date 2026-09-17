use super::*;

#[test]
fn parses_entries_and_flags_k8s_infra() {
    let c = "\
30 25 8:1 / /data rw,relatime shared:1 - ext4 /dev/sda1 rw\n\
31 25 8:1 /sub /var/lib/postgresql rw shared:2 - ext4 /dev/sda1 rw\n\
40 25 0:35 / /etc/hosts rw - tmpfs tmpfs rw\n";
    let e = parse_mountinfo(c);
    assert_eq!(e.len(), 3);
    assert_eq!(
        (e[0].major, e[0].minor, e[0].mount_point.as_str()),
        (8, 1, "/data")
    );
    assert_eq!(e[0].fstype, "ext4");
    assert_eq!(e[0].source, "/dev/sda1");
    assert!(!e[0].is_k8s_infra);
    assert!(e[2].is_k8s_infra); // /etc/hosts
}

#[test]
fn decodes_mountinfo_octal_escapes() {
    let c = "\
30 25 8:1 / /data\\040pg rw,relatime shared:1 - ext4 /dev/disk\\040one rw\n";
    let e = parse_mountinfo(c);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].mount_point, "/data pg");
    assert_eq!(e[0].source, "/dev/disk one");
}

#[test]
fn container_set_excludes_infra_only_devices_and_picks_short_path() {
    let c = "\
30 25 8:1 / /data rw - ext4 /dev/sda1 rw\n\
31 25 8:1 / /data/postgres/pgdata rw - ext4 /dev/sda1 rw\n\
40 25 253:0 / /etc/hosts rw - ext4 /dev/dm-0 rw\n";
    let e = parse_mountinfo(c);
    let set = container_device_set(&e);
    assert!(set.contains(&(8, 1))); // has non-infra mount /data
    assert!(!set.contains(&(253, 0))); // only /etc/hosts -> excluded
    let map = device_map(&e);
    assert_eq!(display_path(&map[&(8, 1)]), Some("/data")); // shortest
}

#[test]
fn kernel_tree_mounts_are_masks_not_filesystems() {
    assert!(is_kernel_tree_mount("/proc/kcore"));
    assert!(is_kernel_tree_mount("/sys/firmware"));
    assert!(is_kernel_tree_mount("/sys"));
    assert!(!is_kernel_tree_mount("/sysroot"));
    assert!(!is_kernel_tree_mount("/var/lib/kronika/data"));
}

#[test]
fn skips_line_without_separator() {
    let c = "30 25 8:1 / /data rw,relatime shared:1 ext4 /dev/sda1 rw\n";
    let e = parse_mountinfo(c);
    assert!(e.is_empty());
}

#[test]
fn device_map_drops_major_zero_but_parse_keeps_it() {
    let c = "40 25 0:35 / /etc/hosts rw - tmpfs tmpfs rw\n";
    let e = parse_mountinfo(c);
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].major, e[0].minor), (0, 35));
    let map = device_map(&e);
    assert!(!map.contains_key(&(0, 35)));
}

#[test]
fn container_set_excludes_all_infra_device() {
    let c = "\
40 25 253:0 / /etc/hosts rw - ext4 /dev/dm-0 rw\n\
41 25 253:0 / /run/secrets/token rw - ext4 /dev/dm-0 rw\n";
    let e = parse_mountinfo(c);
    let set = container_device_set(&e);
    assert!(!set.contains(&(253, 0)));
    assert!(set.is_empty());
}

#[test]
fn display_path_falls_back_to_shortest_when_all_infra() {
    let paths = vec![
        "/run/secrets/kubernetes.io/serviceaccount".to_owned(),
        "/etc/hosts".to_owned(),
    ];
    assert_eq!(display_path(&paths), Some("/etc/hosts"));
}

#[test]
fn mount_row_maps_space_fields() {
    let entry = MountEntry {
        mount_id: 30,
        parent_id: 20,
        major: 8,
        minor: 1,
        root: "/".to_owned(),
        mount_point: "/data".to_owned(),
        fstype: "ext4".to_owned(),
        source: "/dev/sda1".to_owned(),
        deleted: false,
        is_k8s_infra: false,
    };

    let strings = MountStringIds {
        mount_point: StrId(10),
        root: StrId(11),
        fstype: StrId(20),
        source: StrId(30),
    };
    let row = mount_row(&entry, None, 2, 1_000_000, strings);
    assert_eq!(row.total_bytes, None);
    assert_eq!(row.free_bytes, None);
    assert_eq!(row.major, 8);
    assert_eq!(row.minor, 1);
    assert!(!row.is_k8s_infra);
    assert_eq!(row.scope, 2);
    assert_eq!(row.ts, Ts(1_000_000));
    assert_eq!(row.mount_point, StrId(10));
    assert_eq!(row.root, StrId(11));
    assert_eq!(row.fstype, StrId(20));
    assert_eq!(row.source, StrId(30));

    let space = FsSpace {
        total_bytes: 500_000_000,
        free_bytes: 200_000_000,
        total_inodes: 100_000,
        available_inodes: 40_000,
    };
    let row2 = mount_row(&entry, Some(space), 2, 1_000_000, strings);
    assert_eq!(row2.total_bytes, Some(500_000_000));
    assert_eq!(row2.free_bytes, Some(200_000_000));
    assert_eq!(row2.total_inodes, Some(100_000));
    assert_eq!(row2.available_inodes, Some(40_000));
}

use crate::{ProcFs, SysFs};

fn mount_entry(major: i32, minor: i32, source: &str) -> MountEntry {
    MountEntry {
        mount_id: minor,
        parent_id: 1,
        major,
        minor,
        root: "/".to_owned(),
        mount_point: "/data".to_owned(),
        fstype: "btrfs".to_owned(),
        source: source.to_owned(),
        deleted: false,
        is_k8s_infra: false,
    }
}

#[test]
fn resolve_major_zero_rewrites_dev_backed_subvolumes() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("class/block/nvme0n1p2")).expect("mkdir");
    std::fs::write(dir.path().join("class/block/nvme0n1p2/dev"), "259:2\n").expect("write");
    let sys = SysFs::new(dir.path().to_path_buf());

    let mut entries = vec![
        mount_entry(0, 42, "/dev/nvme0n1p2"), // resolvable btrfs subvolume
        mount_entry(0, 43, "tmpfs"),          // no /dev/ source: unchanged
        mount_entry(8, 1, "/dev/sda1"),       // already real: unchanged
    ];
    resolve_major_zero(&sys, &mut entries);

    assert_eq!((entries[0].major, entries[0].minor), (259, 2));
    assert_eq!((entries[1].major, entries[1].minor), (0, 43));
    assert_eq!((entries[2].major, entries[2].minor), (8, 1));
}

#[test]
fn mountinfo_resolves_devices_in_the_supplied_sysfs() {
    let dir = tempfile::tempdir().expect("mount fixture");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("self")).expect("proc fixture");
    std::fs::create_dir_all(sys_root.join("class/block/fixture-disk")).expect("sys fixture");
    std::fs::write(
        proc_root.join("self/mountinfo"),
        "30 25 0:42 / /data rw - btrfs /dev/fixture-disk rw\n",
    )
    .expect("mountinfo");
    std::fs::write(sys_root.join("class/block/fixture-disk/dev"), "259:42\n")
        .expect("device identity");

    let entries =
        collect_entries(&ProcFs::new(proc_root), &SysFs::new(sys_root)).expect("mount entries");

    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].major, entries[0].minor), (259, 42));
}

#[test]
fn resolve_major_zero_leaves_entry_when_sysfs_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sys = SysFs::new(dir.path().to_path_buf());
    let mut entries = vec![mount_entry(0, 42, "/dev/nvme0n1p2")];
    resolve_major_zero(&sys, &mut entries);
    // Unresolvable major==0 stays 0 and is dropped downstream by device_map.
    assert_eq!((entries[0].major, entries[0].minor), (0, 42));
}

#[test]
fn section_conversion_attempts_each_mount_string_and_skips_only_incomplete_rows() {
    let first = mount_entry(8, 1, "/dev/sda1");
    let second = MountEntry {
        mount_point: "/other".to_owned(),
        ..first.clone()
    };
    let mut strings = Vec::new();
    let rows = to_sections(&[first, second], [None, None], 4, 41, |value| {
        strings.push(value.to_owned());
        (value != "/data").then_some(StrId(7))
    });
    assert_eq!(
        strings,
        [
            "/data",
            "/",
            "btrfs",
            "/dev/sda1",
            "/other",
            "/",
            "btrfs",
            "/dev/sda1"
        ]
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].ts.0, rows[0].scope, rows[0].major, rows[0].minor),
        (41, 4, 8, 1)
    );
}
