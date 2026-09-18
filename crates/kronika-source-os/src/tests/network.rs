use super::net_link_facts;
use crate::SysFs;

#[test]
fn net_link_facts_read_sysfs_and_fall_back_to_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rel = "class/net/eno1";
    std::fs::create_dir_all(dir.path().join(rel)).expect("mkdir");
    std::fs::write(dir.path().join(rel).join("speed"), "10000\n").expect("write speed");
    std::fs::write(dir.path().join(rel).join("duplex"), "full\n").expect("write duplex");
    let sys = SysFs::new(dir.path().to_path_buf());

    assert_eq!(net_link_facts(&sys, "eno1"), (Some(10_000), 2));
    // A virtual interface has neither file.
    assert_eq!(net_link_facts(&sys, "lo"), (None, 0));
}

#[test]
fn a_down_interface_reports_no_speed_rather_than_a_negative_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rel = "class/net/eth0";
    std::fs::create_dir_all(dir.path().join(rel)).expect("mkdir");
    std::fs::write(dir.path().join(rel).join("speed"), "-1\n").expect("write speed");
    std::fs::write(dir.path().join(rel).join("duplex"), "unknown\n").expect("write duplex");
    let sys = SysFs::new(dir.path().to_path_buf());

    assert_eq!(net_link_facts(&sys, "eth0"), (None, 0));
}
