use super::{cpu_max_mhz, cpu_numa_node};
use crate::SysFs;

#[test]
fn cpu_max_mhz_reads_sysfs_khz() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rel = "devices/system/cpu/cpu0/cpufreq";
    std::fs::create_dir_all(dir.path().join(rel)).expect("mkdir");
    std::fs::write(dir.path().join(rel).join("cpuinfo_max_freq"), "3600000\n").expect("write");
    let sys = SysFs::new(dir.path().to_path_buf());

    assert_eq!(cpu_max_mhz(&sys, 0), Some(3600.0));
    assert_eq!(cpu_max_mhz(&sys, 1), None);
}

#[test]
fn cpu_numa_node_reads_the_node_symlink_or_reports_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("devices/system/cpu/cpu0/node3")).expect("mkdir");
    std::fs::create_dir_all(dir.path().join("devices/system/cpu/cpu1")).expect("mkdir");
    let sys = SysFs::new(dir.path().to_path_buf());

    assert_eq!(cpu_numa_node(&sys, 0), 3);
    assert_eq!(cpu_numa_node(&sys, 1), -1);
    assert_eq!(cpu_numa_node(&sys, 9), -1);
}

#[test]
fn topology_enrichment_preserves_unknown_values_and_row_local_rejection() {
    let dir = tempfile::tempdir().expect("source fixture");
    let proc = dir.path().join("proc");
    let sys = dir.path().join("sys");
    std::fs::create_dir_all(&proc).expect("proc fixture");
    std::fs::create_dir_all(sys.join("devices/system/cpu/cpu0/node2")).expect("NUMA node");
    std::fs::create_dir_all(sys.join("devices/system/cpu/cpu0/cpufreq")).expect("frequency");
    std::fs::write(
        sys.join("devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"),
        "3200000\n",
    )
    .expect("max frequency");
    std::fs::write(proc.join("cpuinfo"), "processor: 0\nmodel name: first\n\nprocessor: 1\nmodel name: rejected\n\nprocessor: 2\nmodel name: last\n").expect("CPU facts");
    let mut strings = Vec::new();
    let rows = super::collect_topology(
        &crate::ProcFs::new(proc),
        &SysFs::new(sys),
        4,
        41,
        |value| {
            strings.push(value.to_owned());
            (value != "rejected").then_some(kronika_registry::StrId(1))
        },
    )
    .expect("enriched topology");
    assert_eq!(strings, ["first", "rejected", "last"]);
    assert_eq!(
        rows.iter().map(|row| row.cpu_id).collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(
        (
            rows[0].mhz_max,
            rows[0].numa_node,
            rows[0].scope,
            rows[0].ts.0
        ),
        (Some(3200.0), 2, 4, 41)
    );
    assert_eq!(
        (rows[1].mhz_max, rows[1].numa_node, rows[1].core_id),
        (None, -1, -1)
    );
}
