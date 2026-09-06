//! Runs the collector and optional demo workload for a bounded measurement window.
mod report;
mod sample;
mod sections;
mod shutdown;
mod system_activity;
mod workload;

use anyhow::{Context, Result};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use report::Report;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use system_activity::{SystemActivity, SystemActivityConfig};
use workload::{Workload, WorkloadConfig};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn resolve_storage_dir(root: &Path, configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(|| root.join("segments"))
}

fn collector_log_to_stderr(raw: Option<&str>) -> Result<bool> {
    match raw {
        None | Some("file") => Ok(false),
        Some("stderr") => Ok(true),
        Some(value) => anyhow::bail!("KRONIKA_DEMO_COLLECTOR_LOG={value:?} is not file or stderr"),
    }
}

fn collector_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("KRONIKA_COLLECTOR_BIN") {
        return Ok(PathBuf::from(path));
    }
    let here = std::env::current_exe().context("locate the demo binary")?;
    let dir = here
        .parent()
        .context("the demo binary has no parent directory")?;
    Ok(dir.join("kronika-collector"))
}

fn spawn_collector(
    binary: &Path,
    storage_dir: &Path,
    root: &Path,
) -> Result<(std::process::Child, String)> {
    let log_path = root.join("collector.log");
    let log_to_stderr =
        collector_log_to_stderr(std::env::var("KRONIKA_DEMO_COLLECTOR_LOG").ok().as_deref())?;
    let log_description = if log_to_stderr {
        "container stderr".to_owned()
    } else {
        log_path.display().to_string()
    };
    let mut command = Command::new(binary);
    command
        .env("KRONIKA_STORAGE_DIR", storage_dir)
        .stdin(Stdio::null());
    if log_to_stderr {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        let log = std::fs::File::create(&log_path).context("create collector.log")?;
        command
            .stdout(Stdio::from(
                log.try_clone().context("share collector.log for stdout")?,
            ))
            .stderr(Stdio::from(log));
    }
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    Ok((child, log_description))
}

fn measure_segments(root: &Path) -> Result<(usize, u64)> {
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry.file_type().context("stat a data-root entry")?;
            if file_type.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "zms") {
                count += 1;
                bytes += entry.metadata().context("stat a segment")?.len();
            }
        }
    }
    Ok((count, bytes))
}

fn main() -> Result<()> {
    if std::env::args_os().skip(1).eq(["--version"]) {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    anyhow::ensure!(std::env::args_os().len() == 1, "unexpected argument");
    run_demo()
}

fn run_demo() -> Result<()> {
    let root = PathBuf::from(env_or("KRONIKA_DEMO_DIR", "demo-data"));
    let storage_dir = resolve_storage_dir(
        &root,
        std::env::var_os("KRONIKA_STORAGE_DIR").map(PathBuf::from),
    );
    let duration_s: u64 = env_or("KRONIKA_DEMO_DURATION_S", "60")
        .parse()
        .context("KRONIKA_DEMO_DURATION_S is not a u64")?;
    std::fs::create_dir_all(&storage_dir).context("create the demo storage directory")?;

    let stop = shutdown::watch().context("watch for shutdown signals")?;
    let system_activity = SystemActivityConfig::from_env(&root, &storage_dir)
        .context("read the system workload configuration")?
        .map(|config| SystemActivity::start(&config, std::sync::Arc::clone(&stop)));
    let workload = WorkloadConfig::from_env()
        .context("read the workload configuration")?
        .map(Workload::start)
        .transpose()
        .context("start the demo workload")?;

    let binary = collector_binary()?;
    let (mut child, log_description) = spawn_collector(&binary, &storage_dir, &root)?;
    let pid = child.id();
    println!("demo: collector pid {pid} for {duration_s}s, log {log_description}");

    let clock_ticks = rustix::param::clock_ticks_per_second();
    let started = Instant::now();
    // `0` means "run until stopped" rather than an already-elapsed deadline.
    let deadline = (duration_s != 0).then(|| Duration::from_secs(duration_s));
    let mut peak_rss_bytes = 0_u64;
    let mut cpu_ticks = 0_u64;
    loop {
        if deadline.is_some_and(|deadline| started.elapsed() >= deadline) {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
        if let Some(status) = child.try_wait().context("poll the collector")? {
            anyhow::bail!("the collector exited early with {status}; see {log_description}");
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

    if let Some(workload) = workload {
        workload.stop();
    }
    if let Some(system_activity) = system_activity {
        system_activity.stop();
    }

    // Use SIGTERM so the collector flushes its open segment before measurement.
    kill(
        Pid::from_raw(i32::try_from(pid).context("collector pid exceeds i32")?),
        Signal::SIGTERM,
    )
    .context("signal the collector to stop")?;
    let stop_started = Instant::now();
    loop {
        if child.try_wait().context("reap the collector")?.is_some() {
            break;
        }
        anyhow::ensure!(
            stop_started.elapsed() < STOP_TIMEOUT,
            "the collector did not exit within {STOP_TIMEOUT:?} of SIGTERM"
        );
        std::thread::sleep(SAMPLE_INTERVAL);
    }

    let (count, bytes) = measure_segments(&storage_dir)?;
    let journal_bytes = std::fs::metadata(storage_dir.join("active.wal"))
        .map(|meta| meta.len())
        .unwrap_or_default();
    let report = Report {
        duration_s,
        segments: count,
        segment_bytes: bytes,
        journal_bytes,
        peak_rss_bytes,
        cpu_ms: cpu_ticks.saturating_mul(1_000) / clock_ticks.max(1),
        sections: sections::section_rows(&storage_dir)?,
    };
    print!("{}", report.render());
    let report_path = root.join("report.json");
    std::fs::write(&report_path, report.to_json()).context("write report.json")?;
    println!("demo: report {}", report_path.display());
    Ok(())
}

#[cfg(test)]
mod tests;
