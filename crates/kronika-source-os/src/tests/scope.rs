use super::{OsScope, detect_container_from_cgroup, net_scope};

#[test]
fn explicit_proc_root_ignores_the_collectors_container_environment() {
    const CHILD: &str = "KRONIKA_TEST_EXPLICIT_PROC_ROOT";
    if std::env::var_os(CHILD).is_some() {
        let root = tempfile::tempdir().expect("proc fixture");
        std::fs::create_dir(root.path().join("1")).expect("pid 1");
        let membership = root.path().join("1/cgroup");
        std::fs::write(&membership, "0::/init.scope\n").expect("host membership");
        let fs = crate::ProcFs::new(root.path().to_owned());
        assert!(!super::detect_container_with_root_override(&fs, true));
        assert!(super::detect_container_with_root_override(&fs, false));
        std::fs::write(&membership, "0::/kubepods/pod123\n").expect("pod membership");
        assert!(super::detect_container_with_root_override(&fs, true));
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "scope::tests::explicit_proc_root_ignores_the_collectors_container_environment",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("KUBERNETES_SERVICE_HOST", "fixture")
        .env_remove("KRONIKA_PROC_ROOT")
        .output()
        .expect("run isolated container detection");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn scope_encodes_as_stable_u8() {
    // The reader depends on these exact values; guard every variant.
    assert_eq!(OsScope::Host.as_u8(), 0);
    assert_eq!(OsScope::Pod.as_u8(), 1);
    assert_eq!(OsScope::PodNet.as_u8(), 2);
    assert_eq!(OsScope::Container.as_u8(), 3);
    assert_eq!(OsScope::Unknown.as_u8(), 4);
}

#[test]
fn cgroup_markers_detect_a_container() {
    assert!(detect_container_from_cgroup("0::/kubepods/pod123/abc\n"));
    assert!(detect_container_from_cgroup("12:pids:/docker/deadbeef\n"));
    assert!(!detect_container_from_cgroup("0::/init.scope\n"));
}

#[test]
fn net_scope_maps_container_flag() {
    assert_eq!(net_scope(true), OsScope::PodNet);
    assert_eq!(net_scope(false), OsScope::Host);
}
