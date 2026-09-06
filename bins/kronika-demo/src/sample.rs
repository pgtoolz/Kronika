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
mod tests {
    use super::{cpu_ticks, peak_rss_bytes};

    const STATUS: &str = "\
Name:\tkronika-collect
State:\tS (sleeping)
VmPeak:\t  200000 kB
VmSize:\t  180000 kB
VmHWM:\t   12000 kB
VmRSS:\t   11000 kB
";

    // 52 fields; utime = 120 and stime = 30 sit at positions 14 and 15.
    const STAT: &str = "\
4242 (kronika (test) x) S 1 4242 4242 0 -1 4194560 500 0 0 0 120 30 0 0 20 0 5 0 900 \
180000000 11000 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";

    #[test]
    fn peak_rss_comes_from_vmhwm_in_bytes() {
        assert_eq!(peak_rss_bytes(STATUS), Some(12_000 * 1024));
    }

    #[test]
    fn a_status_without_vmhwm_reads_as_unmeasured() {
        assert_eq!(peak_rss_bytes("Name:\tsh\nState:\tS\n"), None);
        assert_eq!(peak_rss_bytes(""), None);
    }

    #[test]
    fn cpu_ticks_sum_user_and_system_past_a_comm_with_spaces_and_parens() {
        assert_eq!(cpu_ticks(STAT), Some(150));
    }

    #[test]
    fn a_truncated_stat_line_reads_as_unmeasured() {
        assert_eq!(cpu_ticks("4242 (sh) S 1 2 3"), None);
        assert_eq!(cpu_ticks("no parenthesis here"), None);
    }
}
