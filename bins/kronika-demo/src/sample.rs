//! Reading a running process's peak footprint and CPU from procfs.

/// Peak resident set of `pid` in bytes, from `VmHWM`.
///
/// `VmHWM` only grows, so the last successful read before the process exits is
/// the peak for the whole run.
pub(crate) fn peak_rss_bytes(status: &str) -> Option<u64> {
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    kib.checked_mul(1024)
}

/// User plus system CPU of `pid` in clock ticks, from fields 14 and 15 of
/// `/proc/PID/stat`.
///
/// The fields are counted from the closing parenthesis of `comm`, because a
/// process name may itself contain spaces and parentheses.
pub(crate) fn cpu_ticks(stat: &str) -> Option<u64> {
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_whitespace();
    // Field 3 (state) is the first token after `comm`; utime is field 14.
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    utime.checked_add(stime)
}

#[cfg(test)]
#[path = "tests/sample.rs"]
mod tests;
