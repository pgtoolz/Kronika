use super::terminate_process_group;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::os::unix::process::CommandExt as _;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn cleanup_stops_the_demo_process_group() {
    let leader = Command::new("sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let process_group = i32::try_from(leader.id()).unwrap();
    // Own both children so the test can reap them. A shell's orphaned `sleep`
    // may still exist when kill(group, 0) runs, even after receiving SIGKILL.
    let mut member = Command::new("sleep")
        .arg("60")
        .process_group(process_group)
        .spawn()
        .unwrap();
    let mut leader = Some(leader);

    terminate_process_group(&mut leader, process_group);

    assert!(leader.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = member.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            drop(member.kill());
            drop(member.wait());
            panic!("process-group cleanup left a member running");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        !status.success(),
        "the group member must have been terminated"
    );
    assert!(kill(Pid::from_raw(-process_group), None).is_err());
}
