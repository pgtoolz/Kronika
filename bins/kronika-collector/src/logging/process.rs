//! Optional CPU and memory diagnostics about the collector process itself.

use std::sync::atomic::{AtomicBool, Ordering};

// PostgreSQL-only mode disables these local Linux reads during startup.
static PROCESS_DIAGNOSTICS: AtomicBool = AtomicBool::new(true);

pub(crate) fn configure_process_diagnostics(enabled: bool) {
    PROCESS_DIAGNOSTICS.store(enabled, Ordering::Relaxed);
}

/// Cumulative process CPU time in clock ticks, available only in local mode.
pub(crate) fn process_cpu_ticks() -> Option<u64> {
    if !PROCESS_DIAGNOSTICS.load(Ordering::Relaxed) {
        return None;
    }
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let stat = kronika_source_os::proc::process::parse_stat(&text).ok()?;
    u64::try_from(stat.utime.checked_add(stat.stime)?).ok()
}

/// Peak resident size in local mode, kibibytes.
pub(crate) fn peak_rss_kib() -> Option<u64> {
    if !PROCESS_DIAGNOSTICS.load(Ordering::Relaxed) {
        return None;
    }
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
}
