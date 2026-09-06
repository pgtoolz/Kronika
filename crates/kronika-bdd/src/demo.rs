use anyhow::{Context as _, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_DEMO_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the BDD binary")?;
    let dir = here
        .parent()
        .context("the BDD binary has no parent directory")?;
    Ok(dir.join("kronika-demo"))
}

#[derive(Debug)]
pub(crate) struct DemoRun {
    root: tempfile::TempDir,
    log_path: PathBuf,
    child: Option<Child>,
    process_group: i32,
    clean_exit: bool,
}

impl DemoRun {
    pub(crate) fn spawn(env: &[(String, String)]) -> Result<Self> {
        let root = tempfile::tempdir().context("create a demo data root")?;
        let log_path = root.path().join("demo.log");
        let log = std::fs::File::create(&log_path).context("create demo.log")?;
        let mut command = Command::new(binary()?);
        command
            .env("KRONIKA_DEMO_DIR", root.path())
            .env_remove("KRONIKA_DEMO_WORKLOAD_DSN")
            .env_remove("KRONIKA_DEMO_WORKLOAD_DIRECT_DSN");
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                log.try_clone().context("share demo.log for stdout")?,
            ))
            .stderr(Stdio::from(log))
            .spawn()
            .context("spawn the demo")?;
        let process_group = i32::try_from(child.id()).context("demo pid exceeds i32")?;
        Ok(Self {
            root,
            log_path,
            child: Some(child),
            process_group,
            clean_exit: false,
        })
    }

    pub(crate) fn wait(&mut self, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        let child = self.child.as_mut().context("the demo was already reaped")?;
        loop {
            if let Some(status) = child.try_wait().context("reap the demo")? {
                self.child = None;
                anyhow::ensure!(status.success(), "the demo exited with {status}");
                self.clean_exit = true;
                return Ok(());
            }
            anyhow::ensure!(
                started.elapsed() < timeout,
                "the demo did not exit within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn log(&self) -> Result<String> {
        std::fs::read_to_string(&self.log_path).context("read demo.log")
    }

    pub(crate) fn data_path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }
}

fn terminate_process_group(child: &mut Option<Child>, process_group: i32) {
    let _ = kill(Pid::from_raw(-process_group), Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(1);
    while child.is_some() && Instant::now() < deadline {
        if child
            .as_mut()
            .is_some_and(|process| process.try_wait().is_ok_and(|status| status.is_some()))
        {
            *child = None;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = kill(Pid::from_raw(-process_group), Signal::SIGKILL);
    if let Some(mut process) = child.take() {
        drop(process.wait());
    }
}

impl Drop for DemoRun {
    fn drop(&mut self) {
        if !self.clean_exit {
            terminate_process_group(&mut self.child, self.process_group);
        }
    }
}

#[cfg(test)]
mod tests;
