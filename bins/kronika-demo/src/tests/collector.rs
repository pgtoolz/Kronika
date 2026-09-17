use super::{Collector, CollectorLog};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicBool;

fn child_fixture(root: &std::path::Path, script: &str) -> std::path::PathBuf {
    let path = root.join("collector");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn collector_early_exit_is_reported_and_reaped() {
    let root = tempfile::tempdir().unwrap();
    let path = child_fixture(root.path(), "exit 7");
    let mut child = Collector::start(&path, root.path(), root.path(), CollectorLog::File).unwrap();
    let error = child.measure(1, &AtomicBool::new(false)).err().unwrap();
    assert!(error.to_string().contains("exited early"));
    child.stop().unwrap();
    assert!(child.child.try_wait().unwrap().is_some());
}

#[test]
fn shutdown_reaps_an_idle_child_without_running_workloads() {
    let root = tempfile::tempdir().unwrap();
    let path = child_fixture(root.path(), "exec sleep 30");
    let mut child = Collector::start(&path, root.path(), root.path(), CollectorLog::File).unwrap();
    child.stop().unwrap();
    assert!(child.child.try_wait().unwrap().is_some());
}

#[test]
fn dropping_the_owner_does_not_leave_the_process_running() {
    let root = tempfile::tempdir().unwrap();
    let path = child_fixture(root.path(), "exec sleep 30");
    let child = Collector::start(&path, root.path(), root.path(), CollectorLog::File).unwrap();
    let pid = nix::unistd::Pid::from_raw(i32::try_from(child.pid()).unwrap());
    drop(child);
    assert_eq!(
        nix::sys::signal::kill(pid, None),
        Err(nix::errno::Errno::ESRCH)
    );
}
