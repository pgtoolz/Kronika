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
    let fs = ProcFs::from_env();
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
mod tests {
    #[cfg(target_os = "linux")]
    use super::collect_os_instance_facts;
    use super::parse_btime;

    #[test]
    fn parse_btime_finds_the_line_between_others() {
        let stat = "cpu  1 2 3 4\nintr 5\nctxt 6\nbtime 1700000000\nprocesses 7\n";
        assert_eq!(parse_btime(stat), Some(1_700_000_000_000_000));
    }

    #[test]
    fn parse_btime_rejects_missing_or_garbled_lines() {
        assert_eq!(parse_btime("cpu 1 2 3\nprocesses 7\n"), None);
        assert_eq!(parse_btime("btime not-a-number\n"), None);
        assert_eq!(parse_btime("btimes 1700000000\n"), None);
        assert_eq!(parse_btime(""), None);
    }

    #[test]
    fn parse_btime_rejects_values_that_overflow_microseconds() {
        assert_eq!(parse_btime("btime 9223372036854776\n"), None);
        assert_eq!(
            parse_btime("btime 9223372036854\n"),
            Some(9_223_372_036_854_000_000)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn collect_reads_the_live_host() {
        let facts = collect_os_instance_facts().expect("running on Linux with /proc");
        assert!(!facts.hostname.is_empty());
        assert!(!facts.kernel_version.is_empty());
        assert_eq!(
            facts.boot_id.len(),
            36,
            "boot_id is a UUID: {}",
            facts.boot_id
        );
        assert!(facts.btime > 0);
        assert!(facts.clock_ticks_per_sec > 0);
        assert!(facts.page_size_bytes >= 4096);
    }
}
