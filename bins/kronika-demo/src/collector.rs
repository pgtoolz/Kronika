//! Own the collector process through startup, measurement, and bounded shutdown.

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::CollectorLog;
use crate::sample;

// Bound procfs polling and graceful-shutdown checks to two reads per second.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
// Give the collector time to finish its in-flight work before shutdown is stuck.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct Measurement {
    pub(crate) peak_rss_bytes: u64,
    pub(crate) cpu_ms: u64,
}

pub(crate) struct Collector {
    child: Child,
    log_description: String,
}

impl Collector {
    pub(crate) fn start(
        binary: &Path,
        storage_dir: &Path,
        root: &Path,
        log: CollectorLog,
    ) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .env("KRONIKA_STORAGE_DIR", storage_dir)
            .stdin(Stdio::null());
        let log_description = match log {
            CollectorLog::Stderr => {
                command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
                "container stderr".to_owned()
            }
            CollectorLog::File => {
                let path = root.join("collector.log");
                let file = std::fs::File::create(&path).context("create collector.log")?;
                command
                    .stdout(Stdio::from(
                        file.try_clone().context("share collector.log for stdout")?,
                    ))
                    .stderr(Stdio::from(file));
                path.display().to_string()
            }
        };
        let child = command
            .spawn()
            .with_context(|| format!("spawn {}", binary.display()))?;
        Ok(Self {
            child,
            log_description,
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn log_description(&self) -> &str {
        &self.log_description
    }

    pub(crate) fn measure(&mut self, duration_s: u64, stop: &AtomicBool) -> Result<Measurement> {
        let started = Instant::now();
        // Zero explicitly selects signal-driven operation.
        let deadline = (duration_s != 0).then(|| Duration::from_secs(duration_s));
        let pid = self.pid();
        let mut peak_rss_bytes = 0;
        let mut cpu_ticks = 0;
        while !stop.load(Ordering::SeqCst)
            && deadline.is_none_or(|deadline| started.elapsed() < deadline)
        {
            std::thread::sleep(SAMPLE_INTERVAL);
            if let Some(status) = self.child.try_wait().context("poll the collector")? {
                anyhow::bail!(
                    "the collector exited early with {status}; see {}",
                    self.log_description
                );
            }
            if let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/status"))
                && let Some(rss) = sample::peak_rss_bytes(&text)
            {
                peak_rss_bytes = peak_rss_bytes.max(rss);
            }
            if let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                && let Some(ticks) = sample::cpu_ticks(&text)
            {
                cpu_ticks = cpu_ticks.max(ticks);
            }
        }
        Ok(Measurement {
            peak_rss_bytes,
            cpu_ms: cpu_ticks.saturating_mul(1_000)
                / rustix::param::clock_ticks_per_second().max(1),
        })
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        if self
            .child
            .try_wait()
            .context("poll the collector before shutdown")?
            .is_some()
        {
            return Ok(());
        }
        // SIGTERM stops collection cleanly; the open segment remains in active.wal.
        kill(
            Pid::from_raw(i32::try_from(self.pid()).context("collector pid exceeds i32")?),
            Signal::SIGTERM,
        )
        .context("signal the collector to stop")?;
        let started = Instant::now();
        loop {
            if self
                .child
                .try_wait()
                .context("reap the collector")?
                .is_some()
            {
                return Ok(());
            }
            anyhow::ensure!(
                started.elapsed() < STOP_TIMEOUT,
                "the collector did not exit within {STOP_TIMEOUT:?} of SIGTERM"
            );
            std::thread::sleep(SAMPLE_INTERVAL);
        }
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        // Error paths and a failed graceful shutdown must not orphan a collector.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            drop(self.child.kill());
            drop(self.child.wait());
        }
    }
}

#[cfg(test)]
#[path = "tests/collector.rs"]
mod tests;
