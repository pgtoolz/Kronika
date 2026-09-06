//! Running the collector binary and reading back what it produced.

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for a clean exit after `SIGTERM`.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// The collector binary under test.
///
/// `KRONIKA_COLLECTOR_BIN` is set by the BDD image; a developer running the
/// suite from a checkout gets the build next to the test binary.
fn binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_COLLECTOR_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the BDD binary")?;
    let dir = here
        .parent()
        .context("the BDD binary has no parent directory")?;
    Ok(dir.join("kronika-collector"))
}

/// One collector run: its data root, its log, and the process while it lives.
#[derive(Debug)]
pub(crate) struct Run {
    root: Option<tempfile::TempDir>,
    log_path: PathBuf,
    child: Option<Child>,
    /// `VmHWM` sampled just before the stop signal, kibibytes.
    peak_rss_kib: Option<u64>,
}

impl Run {
    /// Spawn the collector over a data root with `env` applied on top.
    pub(crate) fn spawn(root: tempfile::TempDir, env: &[(String, String)]) -> Result<Self> {
        let storage_dir = root.path().join("segments");
        std::fs::create_dir_all(&storage_dir).context("create the segments directory")?;
        let log_path = root.path().join("collector.log");
        let log = std::fs::File::create(&log_path).context("create collector.log")?;
        let mut command = Command::new(binary()?);
        command.env("KRONIKA_STORAGE_DIR", &storage_dir);
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                log.try_clone().context("share collector.log for stdout")?,
            ))
            .stderr(Stdio::from(log))
            .spawn()
            .context("spawn the collector")?;
        Ok(Self {
            root: Some(root),
            log_path,
            child: Some(child),
            peak_rss_kib: None,
        })
    }

    /// The collector's data root.
    pub(crate) fn storage_dir(&self) -> PathBuf {
        self.root
            .as_ref()
            .expect("the data root outlives the run")
            .path()
            .join("segments")
    }

    /// Let the collector run, then stop it the way an operator would.
    ///
    /// A collector that already refused startup is simply reaped. Peak RSS is
    /// sampled while a process is still alive; after exit `/proc/<pid>` is gone.
    pub(crate) fn run_for_and_stop(&mut self, duration: Duration) -> Result<()> {
        std::thread::sleep(duration);
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait().context("reap the collector")?.is_some() {
            self.child = None;
            return Ok(());
        }
        let pid = child.id();
        self.peak_rss_kib = read_peak_rss_kib(pid);
        kill(
            Pid::from_raw(i32::try_from(pid).context("collector pid exceeds i32")?),
            Signal::SIGTERM,
        )
        .context("signal the collector to stop")?;
        let started = Instant::now();
        loop {
            if child.try_wait().context("reap the collector")?.is_some() {
                break;
            }
            anyhow::ensure!(
                started.elapsed() < STOP_TIMEOUT,
                "the collector did not exit within {STOP_TIMEOUT:?} of SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        self.child = None;
        Ok(())
    }

    /// Peak resident set size over the run, kibibytes.
    pub(crate) const fn peak_rss_kib(&self) -> Option<u64> {
        self.peak_rss_kib
    }

    /// Hand back the data root so a scenario can restart over it.
    pub(crate) fn into_root(mut self) -> tempfile::TempDir {
        self.child = None;
        self.root.take().expect("the data root outlives the run")
    }

    /// Everything the collector wrote to stdout and stderr.
    pub(crate) fn log(&self) -> Result<String> {
        std::fs::read_to_string(&self.log_path).context("read collector.log")
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // A scenario that failed mid-run leaves the collector alive; the
            // temporary data root goes with it either way.
            drop(child.kill());
            drop(child.wait());
        }
    }
}

/// `VmHWM` from `/proc/<pid>/status`, the kernel's own peak-RSS watermark.
fn read_peak_rss_kib(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Every file under `root`, relative to it.
pub(crate) fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                found.push(relative.to_path_buf());
            }
        }
    }
    found
}

/// Copy a fixture tree into `dest`, creating it.
pub(crate) fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for entry in std::fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)
                .with_context(|| format!("copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}
