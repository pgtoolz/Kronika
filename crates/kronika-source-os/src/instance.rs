//! Host identity facts used by the current `instance_metadata` section.

use crate::ProcFs;
use std::io;

/// Host facts read from `/proc` and `sysconf`.
///
/// These make OS sections self-contained: readers convert tick and page
/// counters without knowing the host configuration, and `boot_id`/`btime`
/// anchor a segment to one boot of one machine.
#[derive(Debug, Clone)]
pub struct OsInstanceFacts {
    /// Kernel node name (`/proc/sys/kernel/hostname`).
    pub hostname: String,
    /// Kernel release string (`/proc/sys/kernel/osrelease`).
    pub kernel_version: String,
    /// Boot UUID (`/proc/sys/kernel/random/boot_id`).
    pub boot_id: String,
    /// Kernel boot time (`/proc/stat` `btime`), unix microseconds.
    pub btime: i64,
    /// `sysconf(_SC_CLK_TCK)`.
    pub clock_ticks_per_sec: i64,
    /// `sysconf(_SC_PAGESIZE)`.
    pub page_size_bytes: i64,
}

/// Read the host facts.
///
/// # Errors
/// Returns an [`io::Error`] naming the `/proc` file that failed to read or
/// parse; the collector runs on Linux, where all of them exist.
pub fn collect_os_instance_facts() -> io::Result<OsInstanceFacts> {
    collect_os_instance_facts_from(&ProcFs::from_env())
}

/// Read identity files from the selected procfs root and clock/page sizes from `sysconf`.
///
/// # Errors
/// Returns an [`io::Error`] when an identity file cannot be read or parsed.
pub fn collect_os_instance_facts_from(fs: &ProcFs) -> io::Result<OsInstanceFacts> {
    let stat = fs.read("stat")?;
    let btime =
        parse_btime(&stat).ok_or_else(|| io::Error::other("stat: no parsable btime line"))?;
    Ok(OsInstanceFacts {
        hostname: fs.read("sys/kernel/hostname")?,
        kernel_version: fs.read("sys/kernel/osrelease")?,
        boot_id: fs.read("sys/kernel/random/boot_id")?,
        btime,
        clock_ticks_per_sec: i64::try_from(rustix::param::clock_ticks_per_second())
            .map_err(io::Error::other)?,
        page_size_bytes: i64::try_from(rustix::param::page_size()).map_err(io::Error::other)?,
    })
}

/// Extract the kernel boot time from `/proc/stat` content, unix microseconds.
fn parse_btime(stat: &str) -> Option<i64> {
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|rest| rest.trim().parse::<i64>().ok())
        .and_then(|secs| secs.checked_mul(1_000_000))
}

#[cfg(test)]
#[path = "tests/instance.rs"]
mod tests;
