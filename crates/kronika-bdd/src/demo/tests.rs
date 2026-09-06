use super::terminate_process_group;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

#[test]
fn cleanup_stops_the_demo_process_group() {
    let child = Command::new("sh")
        .args(["-c", "sleep 60 & wait"])
        .process_group(0)
        .spawn()
        .unwrap();
    let process_group = i32::try_from(child.id()).unwrap();
    let mut child = Some(child);

    terminate_process_group(&mut child, process_group);

    assert!(child.is_none());
    assert!(kill(Pid::from_raw(-process_group), None).is_err());
}
